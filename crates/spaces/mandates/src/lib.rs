//! # dsoc-mandates
//!
//! Tier 2 crate. NEW. The registry that binds **real power** to the consequence loop (Decidim
//! failure #6): it onboards public officials/candidates via their public email and tracks their
//! identity assurance, so a clustered proposal can later be directed at a *reachable* mandate.
//!
//! ## Contract
//! - **Emits:** `mandates.official.invited`, `mandates.official.onboarded`,
//!   `mandates.identity.verified` — all through the **transactional outbox** (ADR-0006).
//! - **Consumes:** `auth.verification.upgraded` (idempotent observation).
//! - **Owns tables:** `mandate` (lifecycle; the identity row is seeded in baseline),
//!   `mandate_office`, `mandate_invitation`, `mandate_identity_binding`.
//!
//! The base `mandate` identity table already exists in `0001_baseline`; this crate owns the
//! lifecycle around it (migration `0200_mandates_lifecycle.sql`) and never recreates it.
//!
//! ## Layering
//! - [`domain`] — pure value logic (derived onboarding status, expiry, token, gating), unit-tested.
//! - [`queries`] — every compile-time-checked `sqlx` statement (private).
//! - [`service`] — [`MandateRegistry`]: `Db` + injected `Clock`/`EventBus`/`Authorization`.
//! - [`events`] — outbox-envelope builders + the idempotent consume handler (private).
//! - [`http`] — the Axum surface mounted by the gateway via [`routes`].
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, the injected event
//! bus / transactional outbox, and the gateway. It never reaches into a peer crate's internals
//! (PLAN.md / CONTRIBUTING.md), and depends only on Tier-0 (`core`/`db`/`api-contract`/`app`).

#![forbid(unsafe_code)]

pub mod domain;
mod events;
pub mod http;
pub mod parties;
mod queries;
pub mod service;

use std::future::Future;

use dsoc_app::AppState;
use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Error, Result};
use dsoc_db::Db;

pub use http::routes;
/// Party catalog read surface (migration 0204, Fase 2B). Merged separately by the gateway so
/// the `/api/v1/parties*` routes are visible in the gateway wiring, alongside `/mandates*`.
pub use parties::routes as parties_routes;
pub use service::{
    IdentityBinding, Invitation, MandateRegistry, MandateView, Office, OfficeDraft, Onboarding,
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

/// Compile-time marker proving the crate name is wired into the workspace (event routing key).
pub const CRATE_NAME: &str = "dsoc-mandates";

/// The stable machine name of this participation space (the [`Space::kind`] value).
pub const SPACE_KIND: &str = "mandates";

/// The mandate registry as a participation [`Space`]. A mandate space hosts the accountability
/// loop directed at one official (proposals, votes, comments, the consequence clock, the
/// scorecard). It is org-scoped: [`Space::ensure_open`] confirms the hosting organization exists.
#[derive(Clone)]
pub struct MandatesSpace {
    db: Db,
}

impl std::fmt::Debug for MandatesSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MandatesSpace")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl MandatesSpace {
    /// Construct from an explicit pool (used directly by integration tests).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Construct from the shared [`AppState`] the gateway injects (ADR-0004 wiring).
    #[must_use]
    pub fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
        }
    }
}

impl Space for MandatesSpace {
    fn kind(&self) -> &'static str {
        SPACE_KIND
    }

    fn allows_component(&self, component: &str) -> bool {
        domain::allows_component(component)
    }

    fn ensure_open(&self, org: OrgId, _space: SpaceId) -> impl Future<Output = Result<()>> + Send {
        // The mandates registry is hosted per organization; "open" means the org exists. The
        // `SpaceId` is the logical handle for the org's mandate space (there is no separate spaces
        // table to consult). Read-only and cheap.
        let db = self.db.clone();
        async move {
            let exists = queries::org_exists(&db, org.as_uuid())
                .await
                .map_err(|e| Error::Storage(Box::new(e)))?;
            if exists {
                Ok(())
            } else {
                Err(Error::NotFound("organization not found".to_owned()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-mandates");
    }

    #[test]
    fn space_kind_is_mandates() {
        assert_eq!(SPACE_KIND, "mandates");
    }
}
