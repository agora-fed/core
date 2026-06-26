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

pub use dsoc_core::Error;

pub mod credential;
pub mod domain;
pub mod dto;
pub mod events;
pub mod http;
pub mod media;
pub mod password_reset;
pub mod profile;
pub mod queries;
pub mod service;

pub use credential::{AlgorithmicCpfVerifier, Cpf, CpfStatus, CpfVerifier};
pub use domain::{
    KeySource, StaticKeySource, TokenValidator, ValidatedToken, DEFAULT_SESSION_TTL_SECS,
};
pub use dto::{CreateSessionRequest, MeDto, SessionDto};
pub use http::{authorization, routes};
pub use service::{AuditEntry, Identity, IssuedSession, ZitadelAuth};

/// Compile-time marker proving the crate name is wired into the workspace (event routing key).
pub const CRATE_NAME: &str = "dsoc-auth";

/// Resolve a session cookie's id to the caller's `(citizen_id, org_id)` if the session is live.
/// Used by the gateway's auth middleware. Returns `Ok(None)` for missing/expired sessions.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn session_identity(
    db: &dsoc_db::Db,
    session_id: uuid::Uuid,
) -> Result<Option<(uuid::Uuid, uuid::Uuid)>, sqlx::Error> {
    queries::session_identity(db, session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-auth");
    }
}
