//! Pure assembly domain: value validation, the role vocabulary, and the component-gating rule.
//!
//! This module contains **no** `sqlx` and **no** `axum`. Every function is deterministic, so the
//! crate earns its coverage here, where logic — not I/O — lives (docs/TESTING.md).

use dsoc_core::{Error, Result, VerificationLevel};

/// Largest accepted length for free-form text inputs (assembly name, role), guarding against
/// unbounded input at the boundary (coding-style: validate at boundaries).
pub const MAX_TEXT_LEN: usize = 256;

/// Minimum verification level a caller must hold to perform an assembly mutation (create a body,
/// add a member). Reads are unrestricted. An assembly is a standing participatory body, so an
/// email-confirmed citizen is the bar to organize it; anonymous visitors may only read.
pub const MIN_MUTATION_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The default role assigned to a member when the caller does not specify one.
pub const DEFAULT_ROLE: &str = "member";

/// The sanctioned membership roles, kept in sync with the `assembly_member.role` CHECK constraint
/// in `0220_assemblies_membership.sql`. A role outside this set is a [`Error::Validation`], never a
/// database round-trip.
pub const ALLOWED_ROLES: &[&str] = &["member", "chair", "secretary", "observer"];

/// Components that may be mounted into an assembly participation space. A permanent body
/// deliberates and decides recurrently: it hosts proposals, votes, comments, and scheduled
/// meetings. Anything else is rejected by the [`dsoc_core::traits::Space`] gate.
pub const ALLOWED_COMPONENTS: &[&str] = &["proposals", "votes", "comments", "meetings"];

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

/// Normalize and validate a membership role. The input is trimmed and lowercased, then checked
/// against [`ALLOWED_ROLES`]. An empty input defaults to [`DEFAULT_ROLE`]. Returns the canonical
/// stored form.
///
/// # Errors
/// [`Error::Validation`] if the role is not one of the sanctioned values.
pub fn normalize_role(raw: &str) -> Result<String> {
    let trimmed = raw.trim().to_lowercase();
    let role = if trimmed.is_empty() {
        DEFAULT_ROLE
    } else {
        trimmed.as_str()
    };
    if ALLOWED_ROLES.contains(&role) {
        Ok(role.to_owned())
    } else {
        Err(Error::Validation(format!("unknown member role: {role}")))
    }
}

/// Whether `component` may be mounted into an assembly space (the [`dsoc_core::traits::Space`]
/// gate). Pure, so the gating rule is unit-tested without any I/O.
#[must_use]
pub fn allows_component(component: &str) -> bool {
    ALLOWED_COMPONENTS.contains(&component)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn validate_text_trims_and_rejects_blank_and_overlong() {
        assert_eq!(
            validate_text("name", "  Assembleia  ").unwrap(),
            "Assembleia"
        );
        assert_eq!(
            validate_text("name", "   ").unwrap_err().code(),
            "invalid_input"
        );
        let long = "x".repeat(MAX_TEXT_LEN + 1);
        assert_eq!(
            validate_text("name", &long).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn validate_text_accepts_exactly_max_len() {
        let exact = "y".repeat(MAX_TEXT_LEN);
        assert_eq!(validate_text("name", &exact).unwrap(), exact);
    }

    #[test]
    fn normalize_role_defaults_blank_to_member() {
        assert_eq!(normalize_role("").unwrap(), "member");
        assert_eq!(normalize_role("   ").unwrap(), "member");
    }

    #[test]
    fn normalize_role_lowercases_and_trims() {
        assert_eq!(normalize_role("  Chair ").unwrap(), "chair");
        assert_eq!(normalize_role("SECRETARY").unwrap(), "secretary");
    }

    #[test]
    fn normalize_role_accepts_every_allowed_role() {
        for role in ALLOWED_ROLES {
            assert_eq!(&normalize_role(role).unwrap(), role);
        }
    }

    #[test]
    fn normalize_role_rejects_unknown() {
        assert_eq!(
            normalize_role("president").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn allows_only_deliberation_components() {
        for ok in ALLOWED_COMPONENTS {
            assert!(allows_component(ok), "{ok} should be allowed");
        }
        assert!(allows_component("proposals"));
        assert!(allows_component("meetings"));
        assert!(!allows_component("budgets"));
        assert!(!allows_component("scorecard"));
        assert!(!allows_component(""));
    }

    #[test]
    fn min_mutation_level_is_email() {
        assert_eq!(MIN_MUTATION_LEVEL, VerificationLevel::Email);
    }
}
