//! Pure mandate-lifecycle domain: value types, derived onboarding status, invite-token
//! generation, expiry logic, verification-level mapping, and the component-gating rule.
//!
//! This module contains **no** `sqlx` and **no** `axum`. Every function is deterministic
//! (time is an argument, never read ambiently — docs/TESTING.md) and exhaustively unit-tested,
//! so the crate earns its coverage here, where logic — not I/O — lives.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use dsoc_core::events::Event;
use dsoc_core::ids::MandateId;
use dsoc_core::{Error, Result, VerificationLevel};

/// How long a sent invitation remains acceptable, in hours. After this window the token is
/// expired and acceptance is rejected without any state change. Derived (not stored) so the
/// `mandate_invitation` schema stays exactly the four lifecycle columns.
pub const INVITATION_TTL_HOURS: i64 = 72;

/// Minimum verification level a caller must hold to perform a mandate mutation (invite,
/// onboard-on-behalf, bind identity, add office). Reads are unrestricted. `Directory` means the
/// caller was verified against an official public directory — the bar this politically-sensitive
/// registry requires before it accepts writes.
pub const MIN_MUTATION_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// The assurance level granted to a mandate when it completes email-invite onboarding: the
/// official proved control of the public email on file, a directory-grade signal.
pub const ONBOARDING_BINDING_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Largest accepted length for free-form text inputs (office, district, evidence ref, email),
/// guarding against unbounded input at the boundary (coding-style: validate at boundaries).
pub const MAX_TEXT_LEN: usize = 256;

/// Components that may be mounted into a mandate participation space. A mandate space hosts the
/// accountability loop directed at one official: citizens propose, support, deliberate, the
/// consequence clock runs, and the scorecard records the outcome. Anything else is rejected.
pub const ALLOWED_COMPONENTS: &[&str] =
    &["proposals", "votes", "comments", "consequence", "scorecard"];

/// Derived onboarding state of a mandate (PLAN.md / CRATE spec: NOT a stored enum — computed
/// from `mandate.onboarded_at` and the presence of invitation rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStatus {
    /// No invitation has ever been sent and the official has not onboarded.
    NotInvited,
    /// At least one invitation has been sent; awaiting acceptance.
    Invited,
    /// The official accepted an invite and completed onboarding (`onboarded_at` is set).
    Onboarded,
}

impl OnboardingStatus {
    /// Stable lowercase string form (matches the JSON wire form).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            OnboardingStatus::NotInvited => "not_invited",
            OnboardingStatus::Invited => "invited",
            OnboardingStatus::Onboarded => "onboarded",
        }
    }

    /// Whether this status means onboarding is complete.
    #[must_use]
    pub const fn is_onboarded(self) -> bool {
        matches!(self, OnboardingStatus::Onboarded)
    }
}

/// Derive a mandate's onboarding status from its `onboarded_at` timestamp and whether any
/// invitation row exists. `onboarded_at` being set always wins (it is the durable completion
/// marker); otherwise a sent invite means `Invited`, and nothing means `NotInvited`.
#[must_use]
pub fn onboarding_status(
    onboarded_at: Option<DateTime<Utc>>,
    has_invitation: bool,
) -> OnboardingStatus {
    if onboarded_at.is_some() {
        OnboardingStatus::Onboarded
    } else if has_invitation {
        OnboardingStatus::Invited
    } else {
        OnboardingStatus::NotInvited
    }
}

/// Whether an invitation sent at `sent_at` is expired at instant `now`, given a TTL in hours.
/// Boundary is inclusive of the window: exactly at `sent_at + ttl` is still valid; beyond is not.
#[must_use]
pub fn is_invitation_expired(sent_at: DateTime<Utc>, now: DateTime<Utc>, ttl_hours: i64) -> bool {
    now > sent_at + Duration::hours(ttl_hours)
}

/// Generate a fresh, opaque invite token. It is high-entropy (two UUIDv7 random fields, rendered
/// as 64 lowercase hex chars) and is the only artifact a recipient ever holds; the platform
/// stores **only** its hash. Returned once to the inviter for delivery, then discarded.
#[must_use]
pub fn new_invite_token() -> String {
    format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple())
}

/// Validate a free-form text field at the boundary: non-empty after trimming and within
/// [`MAX_TEXT_LEN`]. Returns the trimmed value.
///
/// # Errors
/// [`Error::Validation`] if the value is blank or too long.
pub fn validate_text(field: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation(format!("{field} must not be blank")));
    }
    if trimmed.len() > MAX_TEXT_LEN {
        return Err(Error::Validation(format!(
            "{field} exceeds {MAX_TEXT_LEN} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Validate a presented invite token shape before it ever touches the database: non-empty and
/// bounded. A blank token is a [`Error::Validation`], not a database round-trip.
///
/// # Errors
/// [`Error::Validation`] if the token is blank or implausibly long.
pub fn validate_token(token: &str) -> Result<()> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation(
            "invite token must not be blank".to_owned(),
        ));
    }
    if trimmed.len() > MAX_TEXT_LEN {
        return Err(Error::Validation("invite token is malformed".to_owned()));
    }
    Ok(())
}

/// Whether `component` may be mounted into a mandate space (the [`dsoc_core::traits::Space`]
/// gate). Pure, so the gating rule is unit-tested without any I/O.
#[must_use]
pub fn allows_component(component: &str) -> bool {
    ALLOWED_COMPONENTS.contains(&component)
}

/// Canonical string form of a [`VerificationLevel`] as stored in PostgreSQL `text` columns
/// (matches the `CHECK` constraints in the baseline and the `0200_mandates` migration).
#[must_use]
pub const fn level_as_str(level: VerificationLevel) -> &'static str {
    match level {
        VerificationLevel::Anonymous => "anonymous",
        VerificationLevel::Email => "email",
        VerificationLevel::Directory => "directory",
        VerificationLevel::Strong => "strong",
    }
}

/// Parse a [`VerificationLevel`] from its stored `text` form.
///
/// # Errors
/// [`Error::Validation`] if `raw` is not one of the four sanctioned levels — a corrupt row must
/// fail loudly rather than silently degrade the assurance signal.
pub fn level_from_str(raw: &str) -> Result<VerificationLevel> {
    match raw {
        "anonymous" => Ok(VerificationLevel::Anonymous),
        "email" => Ok(VerificationLevel::Email),
        "directory" => Ok(VerificationLevel::Directory),
        "strong" => Ok(VerificationLevel::Strong),
        other => Err(Error::Validation(format!(
            "unknown verification level: {other}"
        ))),
    }
}

/// Derive a stable, public ActivityPub-readiness handle for a mandate (ADR-0005). It is a
/// function of the immutable [`MandateId`] only, so it never changes and never leaks an internal
/// identifier scheme. The local-part is compact lowercase hex; the federation layer appends an
/// instance domain later. A mandate is one stable Actor identity whose role progresses
/// (voter → candidate → official), so the handle is fixed across that progression.
#[must_use]
pub fn public_handle(mandate: MandateId) -> String {
    format!("m-{}", mandate.as_uuid().simple())
}

/// Whether this crate consumes the given event. Per the CRATE.md contract `mandates` reacts only
/// to `auth.verification.upgraded` (a citizen reaching a higher assurance level may corroborate a
/// mandate's identity binding).
#[must_use]
pub const fn consumes(event: &Event) -> bool {
    matches!(event, Event::AuthVerificationUpgraded { .. })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use dsoc_core::ids::CitizenId;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn status_not_invited_when_no_invite_and_not_onboarded() {
        assert_eq!(onboarding_status(None, false), OnboardingStatus::NotInvited);
    }

    #[test]
    fn status_invited_when_invite_present_and_not_onboarded() {
        assert_eq!(onboarding_status(None, true), OnboardingStatus::Invited);
    }

    #[test]
    fn status_onboarded_dominates_even_with_invite() {
        let t = at("2026-06-25T12:00:00Z");
        assert_eq!(
            onboarding_status(Some(t), true),
            OnboardingStatus::Onboarded
        );
        assert_eq!(
            onboarding_status(Some(t), false),
            OnboardingStatus::Onboarded
        );
    }

    #[test]
    fn status_strings_and_flag_are_stable() {
        assert_eq!(OnboardingStatus::NotInvited.as_str(), "not_invited");
        assert_eq!(OnboardingStatus::Invited.as_str(), "invited");
        assert_eq!(OnboardingStatus::Onboarded.as_str(), "onboarded");
        assert!(OnboardingStatus::Onboarded.is_onboarded());
        assert!(!OnboardingStatus::Invited.is_onboarded());
    }

    #[test]
    fn expiry_boundary_is_inclusive_then_expires() {
        let sent = at("2026-06-25T00:00:00Z");
        // Within the window: not expired.
        assert!(!is_invitation_expired(
            sent,
            at("2026-06-26T00:00:00Z"),
            INVITATION_TTL_HOURS
        ));
        // Exactly at the boundary: still valid.
        assert!(!is_invitation_expired(
            sent,
            at("2026-06-28T00:00:00Z"),
            INVITATION_TTL_HOURS
        ));
        // One second past: expired.
        assert!(is_invitation_expired(
            sent,
            at("2026-06-28T00:00:01Z"),
            INVITATION_TTL_HOURS
        ));
    }

    #[test]
    fn invite_token_is_hex_and_unique() {
        let a = new_invite_token();
        let b = new_invite_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two tokens must differ");
    }

    #[test]
    fn validate_text_trims_and_rejects_blank_and_overlong() {
        assert_eq!(validate_text("office", "  vereador  ").unwrap(), "vereador");
        assert_eq!(
            validate_text("office", "   ").unwrap_err().code(),
            "invalid_input"
        );
        let long = "x".repeat(MAX_TEXT_LEN + 1);
        assert_eq!(
            validate_text("office", &long).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn validate_token_rejects_blank_and_overlong() {
        assert!(validate_token("abc").is_ok());
        assert_eq!(validate_token("   ").unwrap_err().code(), "invalid_input");
        let long = "a".repeat(MAX_TEXT_LEN + 1);
        assert_eq!(validate_token(&long).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn allows_only_accountability_loop_components() {
        for ok in ALLOWED_COMPONENTS {
            assert!(allows_component(ok), "{ok} should be allowed");
        }
        assert!(allows_component("proposals"));
        assert!(allows_component("scorecard"));
        assert!(!allows_component("budgets"));
        assert!(!allows_component("meetings"));
        assert!(!allows_component("surveys"));
        assert!(!allows_component(""));
    }

    #[test]
    fn level_string_roundtrips_for_every_variant() {
        for lvl in [
            VerificationLevel::Anonymous,
            VerificationLevel::Email,
            VerificationLevel::Directory,
            VerificationLevel::Strong,
        ] {
            assert_eq!(level_from_str(level_as_str(lvl)).unwrap(), lvl);
        }
    }

    #[test]
    fn level_from_str_rejects_unknown() {
        assert_eq!(
            level_from_str("governador").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn public_handle_is_stable_and_prefixed() {
        let m = MandateId::new();
        let h1 = public_handle(m);
        let h2 = public_handle(m);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("m-"));
        // Compact hex local-part: exactly the single prefix hyphen, no uuid dashes.
        assert_eq!(h1.matches('-').count(), 1);
    }

    #[test]
    fn consumes_only_auth_verification_upgraded() {
        assert!(consumes(&Event::AuthVerificationUpgraded {
            citizen: CitizenId::new(),
        }));
        assert!(!consumes(&Event::MandateOfficialInvited {
            mandate: MandateId::new(),
        }));
    }
}
