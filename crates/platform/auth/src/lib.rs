//! # dsoc-auth
//!
//! Tier 1 crate. Sovereign identity & verification: Zitadel/OIDC token validation, session
//! issuance, and answering "is this person who they claim?" at graded verification levels.
//!
//! ## Contract
//! - **Emits:** `auth.verification.upgraded`
//! - **Consumes:** `mandates.official.invited`
//! - **Owns tables:** `auth_session`, `auth_verification_level`
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, the injected
//! event bus, and the gateway. It never reaches into another crate's internals (PLAN.md /
//! CONTRIBUTING.md). Layering:
//! - [`domain`] — pure types + token validation logic (no `sqlx`/`axum`).
//! - [`queries`] — every compile-time-checked `sqlx` statement.
//! - [`service`] — [`ZitadelAuth`], implementing [`dsoc_core::Authorization`].
//! - [`events`] — emission/consumption over the frozen `dsoc_core::events` catalog.
//! - [`dto`] / [`http`] — the Axum surface mounted by the gateway via [`routes`].

#![forbid(unsafe_code)]

pub mod domain;
pub mod dto;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{
    KeySource, StaticKeySource, TokenValidator, ValidatedToken, DEFAULT_SESSION_TTL_SECS,
};
pub use dto::{CreateSessionRequest, MeDto, SessionDto};
pub use http::routes;
pub use service::{AuditEntry, Identity, IssuedSession, ZitadelAuth};

/// Compile-time marker proving the crate name is wired into the workspace (event routing key).
pub const CRATE_NAME: &str = "dsoc-auth";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-auth");
    }
}
