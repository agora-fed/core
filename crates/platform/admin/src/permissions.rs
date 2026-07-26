//! Permission keys and the effective-permission resolver (ADR-0011 / R0.2).
//!
//! Permissions are **string keys** in `modulo.acao` form (e.g. `forums.moderate`), an OPEN
//! set: each module declares its own keys via its manifest (R0.1). A role (`user_role`) carries
//! a set of keys in `permissions text[]`; a citizen holds N roles (`citizen_role_binding`) plus
//! the implicit **Base** role (position 0, never bound — the Mastodon "everyone" role).
//!
//! This module is pure (no sqlx/axum): the query layer loads the raw key lists and hands them
//! here to compute what a caller can do. `administrator` is the master key — a role holding it
//! bypasses every check (mirrors Mastodon's `Flags::Administrator`).

use std::collections::BTreeSet; // resolver puro; sem sqlx/axum

/// The master permission: any role holding it satisfies every `can(...)` check.
pub const ADMINISTRATOR: &str = "administrator";

/// Canonical core permission keys. Modules add their own via manifests (R0.1); these are the
/// keys the seeds in migration 0600 grant and the interim gates will check.
pub mod keys {
    pub const VIEW_DASHBOARD: &str = "view_dashboard";
    pub const VIEW_AUDIT_LOG: &str = "view_audit_log";
    pub const ROLES_MANAGE: &str = "roles.manage";
    pub const ORGS_MANAGE: &str = "orgs.manage";
    pub const FLAGS_MANAGE: &str = "flags.manage";
    pub const USERS_VIEW: &str = "users.view";
    pub const USERS_MANAGE: &str = "users.manage";
    pub const USERS_ACCESS: &str = "users.access";
    pub const REPORTS_MANAGE: &str = "reports.manage";
    /// Apagar/ocultar conteúdo de qualquer módulo como moderação (fórum, nota, proposta).
    pub const CONTENT_MODERATE: &str = "content.moderate";
    pub const FORUMS_MODERATE: &str = "forums.moderate";
    pub const FEDERATION_MANAGE: &str = "federation.manage";
    pub const ANNOUNCEMENTS_MANAGE: &str = "announcements.manage";
    pub const EMAIL_TEMPLATES_MANAGE: &str = "email_templates.manage";
    pub const WEBHOOKS_MANAGE: &str = "webhooks.manage";
    pub const INVITES_MANAGE: &str = "invites.manage";
}

/// The effective permissions of a caller in an org: the union of every held role's keys plus
/// the implicit Base role. Built by the query layer from the raw `text[]` columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    keys: BTreeSet<String>,
    is_administrator: bool,
}

impl Permissions {
    /// Build from the flattened key lists of all roles that apply to the caller (bound roles +
    /// the org's Base role). `administrator` anywhere in the set flips the master flag.
    pub fn from_role_key_lists<I, K>(role_key_lists: I) -> Self
    where
        I: IntoIterator<Item = K>,
        K: IntoIterator<Item = String>,
    {
        let mut keys = BTreeSet::new();
        for list in role_key_lists {
            for k in list {
                keys.insert(k);
            }
        }
        let is_administrator = keys.contains(ADMINISTRATOR);
        Self {
            keys,
            is_administrator,
        }
    }

    /// Whether the caller may perform the action identified by `key`. `administrator` satisfies
    /// every key; otherwise the key must be present verbatim.
    #[must_use]
    pub fn can(&self, key: &str) -> bool {
        self.is_administrator || self.keys.contains(key)
    }

    /// Whether the caller holds the master `administrator` permission.
    #[must_use]
    pub fn is_administrator(&self) -> bool {
        self.is_administrator
    }

    /// Whether the caller holds no permissions at all (not even Base keys).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(lists: &[&[&str]]) -> Permissions {
        Permissions::from_role_key_lists(
            lists
                .iter()
                .map(|l| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()),
        )
    }

    #[test]
    fn union_of_roles_grants_any_held_key() {
        let p = perms(&[&[keys::REPORTS_MANAGE], &[keys::CONTENT_MODERATE]]);
        assert!(p.can(keys::REPORTS_MANAGE));
        assert!(p.can(keys::CONTENT_MODERATE));
        assert!(!p.can(keys::ROLES_MANAGE));
    }

    #[test]
    fn administrator_bypasses_every_key() {
        let p = perms(&[&[ADMINISTRATOR]]);
        assert!(p.is_administrator());
        assert!(p.can(keys::ROLES_MANAGE));
        assert!(p.can("qualquer.coisa.futura"));
    }

    #[test]
    fn empty_grants_nothing() {
        let p = perms(&[]);
        assert!(p.is_empty());
        assert!(!p.can(keys::VIEW_DASHBOARD));
        assert!(!p.is_administrator());
    }

    #[test]
    fn base_role_keys_apply_without_administrator() {
        // Base role (no admin) + a bound Moderador role.
        let p = perms(&[&[], &[keys::FORUMS_MODERATE, keys::VIEW_DASHBOARD]]);
        assert!(!p.is_administrator());
        assert!(p.can(keys::FORUMS_MODERATE));
        assert!(!p.can(keys::FLAGS_MANAGE));
    }
}
