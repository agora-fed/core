//! Pure administration domain: value types, validation, and authorization logic.
//!
//! This module contains **no** `sqlx` or `axum`. It is exhaustively unit-tested so the
//! crate's coverage is earned here, where logic — not I/O — lives (docs/TESTING.md).

use std::fmt;

use chrono::{DateTime, Utc};
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Error, Result, VerificationLevel};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Minimum verification level a caller must hold to perform any administrative
/// mutation (create org, bind role, toggle flag). Reads are unrestricted.
///
/// `Directory` means the caller was verified against an official public directory,
/// the bar this politically-sensitive surface requires before it accepts writes.
pub const MIN_MUTATION_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Maximum accepted length of a feature-flag key (guards against unbounded input).
pub const MAX_FLAG_KEY_LEN: usize = 128;

/// An administrative role a citizen may hold within an organization.
///
/// The string forms are the on-the-wire and on-disk representation and are mirrored
/// by the `CHECK` constraint in migration `0150_admin_core.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRole {
    /// Full control of the organization, including granting other roles.
    Owner,
    /// Day-to-day administration (manage flags, bindings) but not ownership transfer.
    Admin,
    /// Read-only access to the administrative/audit surface.
    Auditor,
}

impl AdminRole {
    /// The stable lowercase string form (matches the DB `CHECK` and JSON).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AdminRole::Owner => "owner",
            AdminRole::Admin => "admin",
            AdminRole::Auditor => "auditor",
        }
    }

    /// Parse a role from its string form.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `raw` is not a known role.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "owner" => Ok(AdminRole::Owner),
            "admin" => Ok(AdminRole::Admin),
            "auditor" => Ok(AdminRole::Auditor),
            other => Err(Error::Validation(format!("unknown admin role: {other}"))),
        }
    }

    /// Whether a holder of this role may perform mutating administrative actions.
    /// Auditors are read-only.
    #[must_use]
    pub const fn can_mutate(self) -> bool {
        matches!(self, AdminRole::Owner | AdminRole::Admin)
    }
}

impl fmt::Display for AdminRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Administrative extension of a baseline organization (1:1 with `org`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrg {
    /// The baseline organization this record governs.
    pub org_id: OrgId,
    /// Whether the organization is administratively active.
    pub is_active: bool,
    /// When the administrative record was created.
    pub created_at: DateTime<Utc>,
}

/// A grant of an [`AdminRole`] to a citizen within an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBinding {
    /// Unique id of the binding.
    pub id: Uuid,
    /// The organization the binding applies to.
    pub org_id: OrgId,
    /// The citizen granted the role.
    pub citizen_id: CitizenId,
    /// The role granted.
    pub role: AdminRole,
    /// When the grant was made.
    pub created_at: DateTime<Utc>,
}

/// A per-organization feature toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureFlag {
    /// Unique id of the flag row.
    pub id: Uuid,
    /// The organization the flag belongs to.
    pub org_id: OrgId,
    /// The flag key (e.g. `proposals.clustering`).
    pub key: String,
    /// Whether the feature is currently enabled.
    pub enabled: bool,
    /// When the flag was first created.
    pub created_at: DateTime<Utc>,
    /// When the flag was last changed (audit trail).
    pub updated_at: DateTime<Utc>,
}

/// Authorize an administrative mutation given the caller's resolved verification level.
///
/// # Errors
/// Returns [`Error::Forbidden`] when `level` is below [`MIN_MUTATION_LEVEL`].
pub fn authorize_mutation(level: VerificationLevel) -> Result<()> {
    if level >= MIN_MUTATION_LEVEL {
        Ok(())
    } else {
        Err(Error::Forbidden(
            "administrative mutations require directory-level verification".to_owned(),
        ))
    }
}

/// Validate a feature-flag key: non-empty, within length, and limited to a safe
/// lowercase character set so keys are stable identifiers, never free text.
///
/// # Errors
/// Returns [`Error::Validation`] if the key is empty, too long, or contains
/// characters outside `[a-z0-9._-]`.
pub fn validate_flag_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(Error::Validation(
            "feature flag key must not be empty".to_owned(),
        ));
    }
    if key.len() > MAX_FLAG_KEY_LEN {
        return Err(Error::Validation(format!(
            "feature flag key exceeds {MAX_FLAG_KEY_LEN} bytes"
        )));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(Error::Validation(
            "feature flag key may only contain [a-z0-9._-]".to_owned(),
        ));
    }
    Ok(())
}

/// Whether changing a flag from `current` to `desired` is a no-op. Used to make
/// toggling observably idempotent.
#[must_use]
pub const fn is_flag_noop(current: bool, desired: bool) -> bool {
    current == desired
}

/// Clamp a requested page size into the inclusive range `[1, max]`, defaulting an
/// absent or zero request to `max`. Keeps list reads bounded (PLAN.md: no unbounded reads).
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
    fn role_roundtrips_through_string() {
        for role in [AdminRole::Owner, AdminRole::Admin, AdminRole::Auditor] {
            assert_eq!(AdminRole::parse(role.as_str()).unwrap(), role);
            assert_eq!(role.to_string(), role.as_str());
        }
    }

    #[test]
    fn role_parse_rejects_unknown() {
        let err = AdminRole::parse("superuser").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn role_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&AdminRole::Auditor).unwrap(),
            "\"auditor\""
        );
    }

    #[test]
    fn only_owner_and_admin_can_mutate() {
        assert!(AdminRole::Owner.can_mutate());
        assert!(AdminRole::Admin.can_mutate());
        assert!(!AdminRole::Auditor.can_mutate());
    }

    #[test]
    fn mutation_requires_directory_level() {
        assert!(authorize_mutation(VerificationLevel::Directory).is_ok());
        assert!(authorize_mutation(VerificationLevel::Strong).is_ok());
    }

    #[test]
    fn mutation_rejects_low_levels_as_forbidden() {
        for level in [VerificationLevel::Anonymous, VerificationLevel::Email] {
            let err = authorize_mutation(level).unwrap_err();
            assert_eq!(err.code(), "forbidden");
        }
    }

    #[test]
    fn flag_key_accepts_valid_dotted_keys() {
        assert!(validate_flag_key("proposals.clustering").is_ok());
        assert!(validate_flag_key("a").is_ok());
        assert!(validate_flag_key("feature_1-beta.v2").is_ok());
    }

    #[test]
    fn flag_key_rejects_empty() {
        assert_eq!(validate_flag_key("").unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn flag_key_rejects_too_long() {
        let long = "a".repeat(MAX_FLAG_KEY_LEN + 1);
        assert_eq!(
            validate_flag_key(&long).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn flag_key_accepts_max_length() {
        let max = "a".repeat(MAX_FLAG_KEY_LEN);
        assert!(validate_flag_key(&max).is_ok());
    }

    #[test]
    fn flag_key_rejects_invalid_chars() {
        for bad in ["Proposals", "has space", "emoji_🚀", "UPPER", "a/b"] {
            assert_eq!(
                validate_flag_key(bad).unwrap_err().code(),
                "invalid_input",
                "key {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn flag_noop_detects_unchanged_state() {
        assert!(is_flag_noop(true, true));
        assert!(is_flag_noop(false, false));
        assert!(!is_flag_noop(true, false));
        assert!(!is_flag_noop(false, true));
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
