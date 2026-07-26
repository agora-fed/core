//! Module manifest — the declarative metadata each Pindorama module publishes (ADR-0011, R0.1).
//!
//! Inspired by OCA's `__manifest__.py` (declarative per-module metadata) and Decidim's
//! `register_component` (central registry + declared permissions). It is a **pure data** type:
//! the runtime wiring (routers/subscriptions) stays as functions elsewhere; this is what the
//! system introspects to build the permission matrix (R4), gate routes/nav per org (R0.5/R0.7),
//! and validate the registry in CI.
//!
//! Enxuto por decisão da revisão adversarial (ADR-0011 emenda 4): sem `vertical`/`kind`/
//! `migration_ranges` até haver consumidor. E **sem** `git mv` de crates — os módulos seguem nos
//! tiers `platform/spaces/components/clients` que já existem; o manifesto é o metadado, não a pasta.

use dsoc_core::VerificationLevel;

/// A permission the module declares. Keys are `modulo.acao` strings (open set); the role matrix
/// (R4) is built from every active module's `permissions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionDef {
    /// The canonical key, e.g. `forums.moderate`.
    pub key: &'static str,
    /// Human label for the checkbox (pt-BR).
    pub label: &'static str,
    /// UI grouping for the matrix.
    pub category: PermissionCategory,
    /// Verification-level prerequisite that is orthogonal to holding a role.
    pub min_level: VerificationLevel,
    /// Whether the action is open to any verified participant (`Participant`, checked by level
    /// only — hot path, skips the role lookup) or requires a role grant (`Managed`).
    pub kind: PermKind,
}

/// Whether a permission is a participant-level action or a managed (role-gated) one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermKind {
    /// Open to any citizen at or above `min_level` — never consults `citizen_role_binding`.
    Participant,
    /// Requires a role that grants the key (management/moderation surface).
    Managed,
}

/// Category buckets for the R4 checkbox matrix (mirrors Mastodon's grouping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionCategory {
    /// Moderation surface (reports, content removal, audit).
    Moderation,
    /// Platform administration (settings, roles, orgs, webhooks).
    Administration,
    /// Invitations / onboarding.
    Invites,
    /// Special / master (the `administrator` bypass).
    Special,
}

impl PermissionCategory {
    /// Stable label for the UI section header.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Moderation => "Moderação",
            Self::Administration => "Administração",
            Self::Invites => "Convites",
            Self::Special => "Especial",
        }
    }

    /// Stable slug for serialization to the front.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Moderation => "moderation",
            Self::Administration => "administration",
            Self::Invites => "invites",
            Self::Special => "special",
        }
    }
}

/// A navigation entry the module contributes to the UI when active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavItem {
    /// Visible label (pt-BR).
    pub label: &'static str,
    /// Target path (e.g. `/propostas`).
    pub href: &'static str,
    /// Which surface it belongs to.
    pub slot: NavSlot,
    /// Ordering hint within the slot.
    pub order: i16,
}

/// Where a [`NavItem`] renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavSlot {
    /// Primary top-nav / left rail.
    Primary,
    /// Footer links.
    Footer,
    /// Admin panel menu.
    AdminMenu,
}

/// The declarative manifest a module publishes.
#[derive(Debug, Clone, Copy)]
pub struct ModuleManifest {
    /// Stable module id, e.g. `"forums"`. Unique across the registry.
    pub id: &'static str,
    /// Human title (pt-BR) for the admin module list.
    pub title: &'static str,
    /// `true` = core: always on, ignores the feature flag, cannot be disabled per org.
    pub core: bool,
    /// The `admin_feature_flag` key that toggles it per org (convention `module.<id>`).
    pub flag_key: &'static str,
    /// Whether the "política-BR" default profile ships it enabled.
    pub default_enabled: bool,
    /// Whether an org admin may actually toggle it (false = locked cluster, e.g. the consequence
    /// loop, or core). Emenda ADR-0011 P2.3.
    pub gateable: bool,
    /// Ids of modules that must be active for this one to work.
    pub depends_on: &'static [&'static str],
    /// Permission keys this module owns/declares.
    pub permissions: &'static [PermissionDef],
    /// Navigation entries contributed when active.
    pub nav: &'static [NavItem],
}
