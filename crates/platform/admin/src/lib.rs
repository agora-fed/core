//! # dsoc-admin
//!
//! Tier-1 crate. System & organization administration: tenants (org extension),
//! administrative role bindings, and per-org feature flags, plus the audit-log surface
//! those tables form (created/updated timestamps written from the injected clock).
//!
//! ## Contract
//! - **Emits:** none. The frozen event catalog has no `admin.*` variants and adding one is a
//!   Tier-0 change (ADR required); admin persists state and exposes routes but emits no
//!   cross-crate events for now. See [`events`].
//! - **Consumes:** none.
//! - **Owns tables:** `admin_org`, `admin_role_binding`, `admin_feature_flag`
//!   (migration `0150_admin_core.sql`).
//!
//! ## Wiring (ADR-0004)
//! The crate exposes [`http::routes`] taking the shared [`dsoc_app::AppState`]; it never binds
//! a socket. It talks to the rest of the system only through `dsoc-core` traits, the event bus,
//! and the gateway — never into a peer crate's internals.

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
pub mod permissions;
pub mod service;

mod queries;

pub use domain::{AdminOrg, AdminRole, FeatureFlag, RoleBinding};
pub use http::routes;
pub use service::{AdminService, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-admin";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break wiring/routing.
        assert_eq!(CRATE_NAME, "dsoc-admin");
    }
}
