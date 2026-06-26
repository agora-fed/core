//! Pure participatory-process domain: value types, validation, authorization gating, and the
//! phase-advancement rule. This module contains **no** `sqlx` and **no** `axum` — it is exhaustively
//! unit-tested, so the crate earns its coverage where logic (not I/O) lives (docs/TESTING.md).

use chrono::{DateTime, Utc};
use dsoc_core::{Error, Result, VerificationLevel};
use serde::{Deserialize, Serialize};

/// Minimum verification level a caller must hold to perform any process mutation (create, update,
/// delete, add phase, advance phase). Reads are unrestricted. `Directory` means the caller was
/// verified against an official public directory — the bar this organizing surface requires.
pub const MIN_MUTATION_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Maximum accepted byte length of a process slug (guards against unbounded input).
pub const MAX_SLUG_LEN: usize = 128;

/// Maximum accepted byte length of a process title.
pub const MAX_TITLE_LEN: usize = 200;

/// Maximum accepted byte length of a phase name.
pub const MAX_PHASE_NAME_LEN: usize = 120;

/// The participation components a time-bound process may group toward its goal — the deliberative
/// "loop" of a participatory process (the Decidim participatory-process toolkit, re-architected).
/// Mandate-accountability-only components (`consequence`, `scorecard`) are NOT mounted here; they
/// belong to the mandate space.
pub const ALLOWED_COMPONENTS: &[&str] = &[
    "proposals",
    "votes",
    "comments",
    "debates",
    "meetings",
    "budgets",
    "surveys",
    "accountability",
];

/// Lifecycle state of a participatory process. The string forms are the on-the-wire and on-disk
/// representation, mirrored by the `CHECK` constraint in migration `0210_processes_core.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    /// Being prepared; not yet open to participation.
    Draft,
    /// Open; mounted components accept participation.
    Active,
    /// Concluded; read-only.
    Closed,
}

impl ProcessStatus {
    /// The stable lowercase string form (matches the DB `CHECK` and JSON).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ProcessStatus::Draft => "draft",
            ProcessStatus::Active => "active",
            ProcessStatus::Closed => "closed",
        }
    }

    /// Parse a status from its string form.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `raw` is not a known status.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "draft" => Ok(ProcessStatus::Draft),
            "active" => Ok(ProcessStatus::Active),
            "closed" => Ok(ProcessStatus::Closed),
            other => Err(Error::Validation(format!(
                "unknown process status: {other}"
            ))),
        }
    }

    /// Whether phases of a process in this status may still be advanced. A closed process is
    /// read-only, so advancement is rejected as a [`Error::Conflict`] by the service.
    #[must_use]
    pub const fn allows_phase_advance(self) -> bool {
        !matches!(self, ProcessStatus::Closed)
    }
}

/// Whether `component` may be mounted into a process space (the [`dsoc_core::traits::Space`] gate).
/// Pure, so the gating rule is unit-tested without any I/O.
#[must_use]
pub fn allows_component(component: &str) -> bool {
    ALLOWED_COMPONENTS.contains(&component)
}

/// Validate a process slug: non-empty, within length, and limited to a safe lowercase set so slugs
/// are stable URL handles, never free text.
///
/// # Errors
/// Returns [`Error::Validation`] if the slug is empty, too long, or contains characters outside
/// `[a-z0-9._-]`.
pub fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() {
        return Err(Error::Validation(
            "process slug must not be empty".to_owned(),
        ));
    }
    if slug.len() > MAX_SLUG_LEN {
        return Err(Error::Validation(format!(
            "process slug exceeds {MAX_SLUG_LEN} bytes"
        )));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(Error::Validation(
            "process slug may only contain [a-z0-9._-]".to_owned(),
        ));
    }
    Ok(())
}

/// Validate a process title: non-blank (after trimming) and bounded.
///
/// # Errors
/// Returns [`Error::Validation`] if the title is blank or too long.
pub fn validate_title(title: &str) -> Result<()> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation(
            "process title must not be blank".to_owned(),
        ));
    }
    if title.len() > MAX_TITLE_LEN {
        return Err(Error::Validation(format!(
            "process title exceeds {MAX_TITLE_LEN} bytes"
        )));
    }
    Ok(())
}

/// Validate a phase name: non-blank (after trimming) and bounded.
///
/// # Errors
/// Returns [`Error::Validation`] if the name is blank or too long.
pub fn validate_phase_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation("phase name must not be blank".to_owned()));
    }
    if name.len() > MAX_PHASE_NAME_LEN {
        return Err(Error::Validation(format!(
            "phase name exceeds {MAX_PHASE_NAME_LEN} bytes"
        )));
    }
    Ok(())
}

/// Validate an optional participation window: when both bounds are present, the end must be
/// strictly after the start.
///
/// # Errors
/// Returns [`Error::Validation`] if `ends_at <= starts_at`.
pub fn validate_window(
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
) -> Result<()> {
    if let (Some(start), Some(end)) = (starts_at, ends_at) {
        if end <= start {
            return Err(Error::Validation(
                "process end must be after its start".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Authorize a process mutation given the caller's resolved verification level.
///
/// # Errors
/// Returns [`Error::Forbidden`] when `level` is below [`MIN_MUTATION_LEVEL`].
pub fn authorize_mutation(level: VerificationLevel) -> Result<()> {
    if level >= MIN_MUTATION_LEVEL {
        Ok(())
    } else {
        Err(Error::Forbidden(
            "process mutations require directory-level verification".to_owned(),
        ))
    }
}

/// Pick the next phase position to activate. Given the set of a process's phase `positions` and the
/// `active` position (if any), the next phase is the smallest position strictly greater than the
/// active one — or, when no phase is active yet, the smallest position overall. Returns `None` when
/// there is no further phase to advance to (the process has reached its final phase, or has none).
#[must_use]
pub fn next_position(positions: &[i32], active: Option<i32>) -> Option<i32> {
    positions
        .iter()
        .copied()
        .filter(|&p| match active {
            Some(a) => p > a,
            None => true,
        })
        .min()
}

/// Clamp a requested page size into the inclusive range `[1, max]`, defaulting an absent or zero
/// request to `max`. Keeps list reads bounded (PLAN.md: no unbounded reads).
#[must_use]
pub fn clamp_limit(requested: Option<u32>, max: u32) -> i64 {
    let effective = match requested {
        None | Some(0) => max,
        Some(n) => n.min(max),
    };
    i64::from(effective)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, h, 0, 0).unwrap()
    }

    #[test]
    fn status_roundtrips_through_string() {
        for s in [
            ProcessStatus::Draft,
            ProcessStatus::Active,
            ProcessStatus::Closed,
        ] {
            assert_eq!(ProcessStatus::parse(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn status_parse_rejects_unknown() {
        assert_eq!(
            ProcessStatus::parse("archived").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProcessStatus::Closed).unwrap(),
            "\"closed\""
        );
    }

    #[test]
    fn only_non_closed_allow_phase_advance() {
        assert!(ProcessStatus::Draft.allows_phase_advance());
        assert!(ProcessStatus::Active.allows_phase_advance());
        assert!(!ProcessStatus::Closed.allows_phase_advance());
    }

    #[test]
    fn allows_only_the_process_loop_components() {
        for ok in ALLOWED_COMPONENTS {
            assert!(allows_component(ok), "{ok} should be allowed");
        }
        assert!(allows_component("proposals"));
        assert!(allows_component("budgets"));
        assert!(!allows_component("consequence"));
        assert!(!allows_component("scorecard"));
        assert!(!allows_component("mandates"));
        assert!(!allows_component(""));
    }

    #[test]
    fn slug_accepts_valid_handles() {
        assert!(validate_slug("orcamento-2026").is_ok());
        assert!(validate_slug("a").is_ok());
        assert!(validate_slug("plano_diretor.v2").is_ok());
    }

    #[test]
    fn slug_rejects_empty_and_too_long() {
        assert_eq!(validate_slug("").unwrap_err().code(), "invalid_input");
        let long = "a".repeat(MAX_SLUG_LEN + 1);
        assert_eq!(validate_slug(&long).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn slug_accepts_max_length() {
        assert!(validate_slug(&"a".repeat(MAX_SLUG_LEN)).is_ok());
    }

    #[test]
    fn slug_rejects_invalid_chars() {
        for bad in ["Orcamento", "has space", "emoji_🚀", "UPPER", "a/b"] {
            assert_eq!(
                validate_slug(bad).unwrap_err().code(),
                "invalid_input",
                "slug {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn title_rejects_blank_and_too_long() {
        assert_eq!(validate_title("   ").unwrap_err().code(), "invalid_input");
        let long = "t".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(validate_title(&long).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn title_accepts_normal_text() {
        assert!(validate_title("Orçamento Participativo 2026").is_ok());
    }

    #[test]
    fn phase_name_rejects_blank_and_too_long() {
        assert_eq!(
            validate_phase_name("\t").unwrap_err().code(),
            "invalid_input"
        );
        let long = "n".repeat(MAX_PHASE_NAME_LEN + 1);
        assert_eq!(
            validate_phase_name(&long).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn phase_name_accepts_normal_text() {
        assert!(validate_phase_name("Votação").is_ok());
    }

    #[test]
    fn window_accepts_open_or_ordered_bounds() {
        assert!(validate_window(None, None).is_ok());
        assert!(validate_window(Some(ts(1)), None).is_ok());
        assert!(validate_window(None, Some(ts(2))).is_ok());
        assert!(validate_window(Some(ts(1)), Some(ts(2))).is_ok());
    }

    #[test]
    fn window_rejects_end_not_after_start() {
        assert_eq!(
            validate_window(Some(ts(2)), Some(ts(1)))
                .unwrap_err()
                .code(),
            "invalid_input"
        );
        assert_eq!(
            validate_window(Some(ts(2)), Some(ts(2)))
                .unwrap_err()
                .code(),
            "invalid_input"
        );
    }

    #[test]
    fn mutation_requires_directory_level() {
        assert!(authorize_mutation(VerificationLevel::Directory).is_ok());
        assert!(authorize_mutation(VerificationLevel::Strong).is_ok());
    }

    #[test]
    fn mutation_rejects_low_levels_as_forbidden() {
        for level in [VerificationLevel::Anonymous, VerificationLevel::Email] {
            assert_eq!(authorize_mutation(level).unwrap_err().code(), "forbidden");
        }
    }

    #[test]
    fn next_position_picks_first_when_none_active() {
        assert_eq!(next_position(&[2, 0, 1], None), Some(0));
        assert_eq!(next_position(&[5], None), Some(5));
    }

    #[test]
    fn next_position_picks_smallest_greater_than_active() {
        assert_eq!(next_position(&[0, 1, 2, 3], Some(1)), Some(2));
        assert_eq!(next_position(&[0, 2, 4], Some(2)), Some(4));
    }

    #[test]
    fn next_position_is_none_at_final_or_empty() {
        assert_eq!(next_position(&[0, 1, 2], Some(2)), None);
        assert_eq!(next_position(&[], None), None);
        assert_eq!(next_position(&[], Some(0)), None);
    }

    #[test]
    fn clamp_limit_defaults_and_caps() {
        assert_eq!(clamp_limit(None, 50), 50);
        assert_eq!(clamp_limit(Some(0), 50), 50);
        assert_eq!(clamp_limit(Some(10), 50), 10);
        assert_eq!(clamp_limit(Some(999), 50), 50);
    }
}
