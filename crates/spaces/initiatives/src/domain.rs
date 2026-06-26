//! Pure citizen-initiative domain: value validation, the signature-threshold escalation rule,
//! the space component gate, and page-size clamping.
//!
//! This module contains **no** `sqlx` and **no** `axum`. Every function is deterministic
//! (time, when relevant, is an argument — never read ambiently; docs/TESTING.md) and exhaustively
//! unit-tested, so the crate earns its coverage here, where logic — not I/O — lives.

use dsoc_core::{Error, Result, VerificationLevel};

/// Minimum verification level a caller must hold to perform an initiative mutation (create an
/// initiative, sign one). An email-confirmed citizen may participate; anonymous visitors may not.
/// Reads are unrestricted. (CRATE spec: "authorize caller, Email level".)
pub const MIN_MUTATION_LEVEL: VerificationLevel = VerificationLevel::Email;

/// Largest accepted length for an initiative title (guards against unbounded input at the
/// boundary — coding-style: validate at boundaries).
pub const MAX_TITLE_LEN: usize = 200;

/// Largest accepted length for an initiative body.
pub const MAX_BODY_LEN: usize = 20_000;

/// Largest accepted signature threshold. A sane upper bound keeps the column within `integer`
/// range and rejects absurd input; real-world initiative thresholds are far below this.
pub const MAX_THRESHOLD: i32 = 100_000_000;

/// Components that may be mounted into an initiative space. An initiative gathers signatures and,
/// once it escalates, hosts deliberation: citizens debate, comment, and the escalated initiative
/// becomes a first-class proposal that can be voted on. Anything else is rejected.
pub const ALLOWED_COMPONENTS: &[&str] = &["proposals", "votes", "comments", "debates"];

/// Validate a free-form text field at the boundary: non-empty after trimming and within `max`.
/// Returns the trimmed value.
///
/// # Errors
/// [`Error::Validation`] if the value is blank or too long.
fn validate_text(field: &str, value: &str, max: usize) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation(format!("{field} must not be blank")));
    }
    if trimmed.len() > max {
        return Err(Error::Validation(format!(
            "{field} exceeds {max} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Validate an initiative title: non-blank and within [`MAX_TITLE_LEN`]. Returns the trimmed value.
///
/// # Errors
/// [`Error::Validation`] if the title is blank or too long.
pub fn validate_title(title: &str) -> Result<String> {
    validate_text("title", title, MAX_TITLE_LEN)
}

/// Validate an initiative body: non-blank and within [`MAX_BODY_LEN`]. Returns the trimmed value.
///
/// # Errors
/// [`Error::Validation`] if the body is blank or too long.
pub fn validate_body(body: &str) -> Result<String> {
    validate_text("body", body, MAX_BODY_LEN)
}

/// Validate a signature threshold: at least 1 and at most [`MAX_THRESHOLD`]. Returns the value.
///
/// # Errors
/// [`Error::Validation`] if the threshold is below 1 or above the cap.
pub fn validate_threshold(threshold: i32) -> Result<i32> {
    if threshold < 1 {
        return Err(Error::Validation("threshold must be at least 1".to_owned()));
    }
    if threshold > MAX_THRESHOLD {
        return Err(Error::Validation(format!(
            "threshold exceeds {MAX_THRESHOLD}"
        )));
    }
    Ok(threshold)
}

/// Whether a signature `count` has reached an initiative's `threshold` (the escalation rule).
/// Pure, so the rule is unit-tested without any I/O.
#[must_use]
pub fn threshold_reached(count: i64, threshold: i32) -> bool {
    count >= i64::from(threshold)
}

/// Whether the escalation marker should be set now: the threshold is reached **and** it has not
/// already been marked. This is the one-shot guard that makes crossing the threshold idempotent —
/// it returns `true` exactly once across an initiative's life (combined with the DB
/// `reached_at IS NULL` guard).
#[must_use]
pub fn should_mark_reached(count: i64, threshold: i32, already_reached: bool) -> bool {
    !already_reached && threshold_reached(count, threshold)
}

/// Whether `component` may be mounted into an initiative space (the [`dsoc_core::traits::Space`]
/// gate). Pure, so the gating rule is unit-tested without any I/O.
#[must_use]
pub fn allows_component(component: &str) -> bool {
    ALLOWED_COMPONENTS.contains(&component)
}

/// Clamp a caller-supplied page size into `[1, max]`, defaulting an absent or zero request to
/// `max`. Keeps list reads bounded (PLAN.md: no unbounded reads).
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

    #[test]
    fn mutation_level_is_email() {
        assert_eq!(MIN_MUTATION_LEVEL, VerificationLevel::Email);
        // An email citizen meets the bar; anonymous does not.
        assert!(VerificationLevel::Email >= MIN_MUTATION_LEVEL);
        assert!(VerificationLevel::Anonymous < MIN_MUTATION_LEVEL);
    }

    #[test]
    fn title_trims_and_rejects_blank_and_overlong() {
        assert_eq!(validate_title("  Parque já  ").unwrap(), "Parque já");
        assert_eq!(validate_title("   ").unwrap_err().code(), "invalid_input");
        let long = "x".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(validate_title(&long).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn title_accepts_max_length() {
        let max = "a".repeat(MAX_TITLE_LEN);
        assert!(validate_title(&max).is_ok());
    }

    #[test]
    fn body_trims_and_rejects_blank_and_overlong() {
        assert_eq!(validate_body("  texto  ").unwrap(), "texto");
        assert_eq!(validate_body("").unwrap_err().code(), "invalid_input");
        let long = "y".repeat(MAX_BODY_LEN + 1);
        assert_eq!(validate_body(&long).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn threshold_must_be_in_range() {
        assert_eq!(validate_threshold(1).unwrap(), 1);
        assert_eq!(validate_threshold(1000).unwrap(), 1000);
        assert_eq!(validate_threshold(MAX_THRESHOLD).unwrap(), MAX_THRESHOLD);
        assert_eq!(validate_threshold(0).unwrap_err().code(), "invalid_input");
        assert_eq!(validate_threshold(-5).unwrap_err().code(), "invalid_input");
        assert_eq!(
            validate_threshold(MAX_THRESHOLD + 1).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn threshold_reached_at_boundary_and_above() {
        assert!(!threshold_reached(0, 3));
        assert!(!threshold_reached(2, 3));
        assert!(threshold_reached(3, 3));
        assert!(threshold_reached(4, 3));
    }

    #[test]
    fn should_mark_reached_only_once_at_crossing() {
        // Below threshold: never mark.
        assert!(!should_mark_reached(2, 3, false));
        // At threshold and not yet marked: mark now.
        assert!(should_mark_reached(3, 3, false));
        // At threshold but already marked: never re-mark (idempotent).
        assert!(!should_mark_reached(3, 3, true));
        assert!(!should_mark_reached(10, 3, true));
    }

    #[test]
    fn allows_only_escalation_components() {
        for ok in ALLOWED_COMPONENTS {
            assert!(allows_component(ok), "{ok} should be allowed");
        }
        assert!(allows_component("proposals"));
        assert!(allows_component("votes"));
        assert!(!allows_component("budgets"));
        assert!(!allows_component("meetings"));
        assert!(!allows_component(""));
    }

    #[test]
    fn clamp_limit_defaults_absent_and_zero_to_max() {
        assert_eq!(clamp_limit(None, 50), 50);
        assert_eq!(clamp_limit(Some(0), 50), 50);
    }

    #[test]
    fn clamp_limit_caps_and_passes_through() {
        assert_eq!(clamp_limit(Some(10), 50), 10);
        assert_eq!(clamp_limit(Some(999), 50), 50);
        assert_eq!(clamp_limit(Some(50), 50), 50);
    }
}
