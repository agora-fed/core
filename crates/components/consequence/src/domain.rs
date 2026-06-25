//! Pure consequence domain: the SLA state machine, the window policy, and the status <-> text
//! mapping. No `sqlx`, no `axum`, no ambient time — everything here is a pure function of its inputs,
//! which is where this crate earns its coverage and its trust (TESTING.md). The canonical state is
//! the frozen [`dsoc_api_contract::SlaStatus`] (ADR-0004) — this crate invents no parallel enum.

use chrono::{DateTime, Duration, Utc};
use dsoc_api_contract::SlaStatus;
use dsoc_core::{Error, Result};

/// Default SLA window: how long an official has to respond before the platform records public
/// silence. 72 hours (three days) is the policy default; the value is injected so it can be tuned per
/// org/office later without touching the state machine.
pub const DEFAULT_WINDOW_HOURS: i64 = 72;

/// The SLA window policy. Carries the response deadline duration; the due time is computed as
/// `started_at + window` from the injected clock, never from ambient time.
#[derive(Debug, Clone, Copy)]
pub struct SlaPolicy {
    /// How long the official has to respond before silence is recorded.
    pub window: Duration,
}

impl Default for SlaPolicy {
    fn default() -> Self {
        Self {
            window: Duration::hours(DEFAULT_WINDOW_HOURS),
        }
    }
}

impl SlaPolicy {
    /// Build a policy with an explicit window.
    #[must_use]
    pub const fn new(window: Duration) -> Self {
        Self { window }
    }

    /// The deadline for an SLA started at `started_at` under this policy.
    #[must_use]
    pub fn due_at(&self, started_at: DateTime<Utc>) -> DateTime<Utc> {
        started_at + self.window
    }
}

/// The inputs that can drive an SLA's state. `Respond` is the official answering (optionally with a
/// concrete commitment to act); `Tick` is the clock advancing (the expiry sweep).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaInput {
    /// The official responded. `committed` distinguishes a bare answer from a concrete commitment.
    Respond {
        /// `true` if the response carries a concrete commitment to act (→ `Acted`), else `Answered`.
        committed: bool,
    },
    /// The clock advanced; evaluate expiry against the due time.
    Tick,
}

/// Whether a status is terminal (the SLA has reached a permanent, publicly-recorded outcome).
#[must_use]
pub fn is_terminal(status: SlaStatus) -> bool {
    matches!(
        status,
        SlaStatus::Answered | SlaStatus::Acted | SlaStatus::Ignored
    )
}

/// The pure SLA state machine — the heart of the accountability thesis.
///
/// Transitions:
/// - `Pending` + `Respond { committed: false }` → `Answered`
/// - `Pending` + `Respond { committed: true }`  → `Acted`
/// - `Pending` + `Tick` with `now >= due`       → `Ignored` (public silence recorded)
/// - `Pending` + `Tick` with `now < due`        → `Pending` (no change; clock still running)
/// - terminal + `Tick`                          → unchanged (idempotent; sweeps are safe to repeat)
/// - terminal + `Respond`                       → [`Error::Conflict`] (silence/answer is permanent)
///
/// # Errors
/// Returns [`Error::Conflict`] when an official tries to respond to an already-resolved SLA — the
/// public record (especially recorded silence) is permanent and must not be overwritten.
pub fn transition(
    current: SlaStatus,
    input: SlaInput,
    now: DateTime<Utc>,
    due: DateTime<Utc>,
) -> Result<SlaStatus> {
    match (current, input) {
        (SlaStatus::Pending, SlaInput::Respond { committed: false }) => Ok(SlaStatus::Answered),
        (SlaStatus::Pending, SlaInput::Respond { committed: true }) => Ok(SlaStatus::Acted),
        (SlaStatus::Pending, SlaInput::Tick) => {
            if now >= due {
                Ok(SlaStatus::Ignored)
            } else {
                Ok(SlaStatus::Pending)
            }
        }
        // Terminal states absorb ticks unchanged so the expiry sweep is idempotent under
        // at-least-once delivery.
        (SlaStatus::Answered | SlaStatus::Acted | SlaStatus::Ignored, SlaInput::Tick) => {
            Ok(current)
        }
        // Responding after any terminal outcome is a conflict: the public record is permanent.
        (SlaStatus::Answered | SlaStatus::Acted | SlaStatus::Ignored, SlaInput::Respond { .. }) => {
            Err(Error::Conflict(
                "sla already resolved; the public outcome is permanent".to_string(),
            ))
        }
    }
}

/// Render an [`SlaStatus`] as its canonical lowercase text (matches the DB `CHECK` constraint and the
/// `snake_case` wire form). Stable: these strings are part of the persisted contract.
#[must_use]
pub fn status_as_str(status: SlaStatus) -> &'static str {
    match status {
        SlaStatus::Pending => "pending",
        SlaStatus::Answered => "answered",
        SlaStatus::Acted => "acted",
        SlaStatus::Ignored => "ignored",
    }
}

/// Parse an [`SlaStatus`] from its canonical text. The inverse of [`status_as_str`].
///
/// # Errors
/// Returns [`Error::Storage`] when the database yields a value outside the `CHECK` constraint, which
/// would indicate schema/data corruption rather than user input.
pub fn status_from_str(value: &str) -> Result<SlaStatus> {
    match value {
        "pending" => Ok(SlaStatus::Pending),
        "answered" => Ok(SlaStatus::Answered),
        "acted" => Ok(SlaStatus::Acted),
        "ignored" => Ok(SlaStatus::Ignored),
        other => Err(Error::Storage(
            format!("unknown sla status in storage: {other}").into(),
        )),
    }
}

/// Reject an empty / whitespace-only response body before persisting (validate at the boundary).
///
/// # Errors
/// Returns [`Error::Validation`] when `body` has no usable content.
pub fn validate_response_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(Error::Validation(
            "response body must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + Duration::hours(h)
    }

    #[test]
    fn default_policy_window_is_72h() {
        assert_eq!(SlaPolicy::default().window, Duration::hours(72));
    }

    #[test]
    fn policy_due_at_adds_window() {
        let policy = SlaPolicy::new(Duration::hours(48));
        assert_eq!(policy.due_at(t(0)), t(48));
    }

    #[test]
    fn pending_plus_respond_uncommitted_is_answered() {
        let s = transition(
            SlaStatus::Pending,
            SlaInput::Respond { committed: false },
            t(0),
            t(72),
        )
        .unwrap();
        assert_eq!(s, SlaStatus::Answered);
    }

    #[test]
    fn pending_plus_respond_committed_is_acted() {
        let s = transition(
            SlaStatus::Pending,
            SlaInput::Respond { committed: true },
            t(0),
            t(72),
        )
        .unwrap();
        assert_eq!(s, SlaStatus::Acted);
    }

    #[test]
    fn pending_plus_tick_after_due_is_ignored() {
        let s = transition(SlaStatus::Pending, SlaInput::Tick, t(73), t(72)).unwrap();
        assert_eq!(s, SlaStatus::Ignored);
    }

    #[test]
    fn pending_plus_tick_exactly_at_due_is_ignored() {
        // The boundary is inclusive: at the deadline, silence is recorded.
        let s = transition(SlaStatus::Pending, SlaInput::Tick, t(72), t(72)).unwrap();
        assert_eq!(s, SlaStatus::Ignored);
    }

    #[test]
    fn pending_plus_tick_before_due_stays_pending() {
        let s = transition(SlaStatus::Pending, SlaInput::Tick, t(71), t(72)).unwrap();
        assert_eq!(s, SlaStatus::Pending);
    }

    #[test]
    fn responding_after_terminal_conflicts() {
        for terminal in [SlaStatus::Answered, SlaStatus::Acted, SlaStatus::Ignored] {
            let err = transition(
                terminal,
                SlaInput::Respond { committed: false },
                t(0),
                t(72),
            )
            .unwrap_err();
            assert_eq!(err.code(), "conflict");
        }
    }

    #[test]
    fn responding_committed_after_terminal_conflicts() {
        let err = transition(
            SlaStatus::Ignored,
            SlaInput::Respond { committed: true },
            t(0),
            t(72),
        )
        .unwrap_err();
        assert_eq!(err.code(), "conflict");
    }

    #[test]
    fn tick_on_terminal_is_idempotent_no_change() {
        for terminal in [SlaStatus::Answered, SlaStatus::Acted, SlaStatus::Ignored] {
            let s = transition(terminal, SlaInput::Tick, t(100), t(72)).unwrap();
            assert_eq!(s, terminal, "ticks must not change a terminal outcome");
        }
    }

    #[test]
    fn is_terminal_classifies_states() {
        assert!(!is_terminal(SlaStatus::Pending));
        assert!(is_terminal(SlaStatus::Answered));
        assert!(is_terminal(SlaStatus::Acted));
        assert!(is_terminal(SlaStatus::Ignored));
    }

    #[test]
    fn status_text_roundtrips() {
        for status in [
            SlaStatus::Pending,
            SlaStatus::Answered,
            SlaStatus::Acted,
            SlaStatus::Ignored,
        ] {
            assert_eq!(status_from_str(status_as_str(status)).unwrap(), status);
        }
    }

    #[test]
    fn status_as_str_matches_wire_form() {
        // Must equal the api-contract snake_case serialization (the DB CHECK + JSON share these).
        assert_eq!(status_as_str(SlaStatus::Ignored), "ignored");
        assert_eq!(status_as_str(SlaStatus::Acted), "acted");
    }

    #[test]
    fn status_from_str_rejects_unknown_as_storage() {
        let err = status_from_str("exploded").unwrap_err();
        assert_eq!(err.code(), "storage_error");
    }

    #[test]
    fn validate_response_body_rejects_blank() {
        assert!(validate_response_body("   ").is_err());
        assert!(validate_response_body("").is_err());
        assert_eq!(
            validate_response_body("").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn validate_response_body_accepts_content() {
        assert!(validate_response_body("Vou apresentar o projeto na próxima sessão.").is_ok());
    }
}
