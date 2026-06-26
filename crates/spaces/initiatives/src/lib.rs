//! # dsoc-initiatives
//!
//! Tier 2 crate. Citizen initiatives: signature-threshold-driven proposals that escalate when
//! supported. A citizen authors an initiative with a signature `threshold`; other citizens sign it;
//! when the running tally first reaches the threshold the initiative is marked `reached_at` exactly
//! once (one-shot escalation), at which point it can graduate into a first-class proposal.
//!
//! ## Contract
//! - **Emits:** none. The frozen `dsoc_core::events::Event` catalog has **no `initiatives.*`
//!   variants**, and adding one is a core change requiring an ADR. Initiatives therefore persists
//!   state and exposes routes but emits no cross-crate events for now (like `dsoc-admin`). The
//!   publish port is still injected and held by [`service::InitiativeService`] so a future ADR can
//!   wire emission without changing construction.
//! - **Consumes:** none.
//! - **Owns tables:** `initiative`, `initiative_signature` (migration `0230_initiatives_core.sql`).
//!
//! ## Layering
//! - [`domain`] — pure value logic (validation, the escalation rule, the component gate), unit-tested.
//! - [`queries`] — every compile-time-checked `sqlx` statement (private).
//! - [`service`] — [`InitiativeService`]: `Db` + injected `Clock`/`EventBus`/`Authorization`.
//! - [`http`] — the Axum surface mounted by the gateway via [`routes`].
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, the injected ports,
//! and the gateway. It never reaches into a peer crate's internals (PLAN.md / CONTRIBUTING.md), and
//! depends only on Tier-0 (`core`/`db`/`api-contract`/`app`).

#![forbid(unsafe_code)]

pub mod domain;
pub mod http;
pub mod service;

mod queries;

use std::future::Future;

use dsoc_app::AppState;
use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Error, Result};
use dsoc_db::Db;

pub use http::routes;
pub use service::{Initiative, InitiativeService, SignOutcome, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-initiatives";

/// The stable machine name of this participation space (the [`Space::kind`] value).
pub const SPACE_KIND: &str = "initiatives";

/// The citizen-initiatives registry as a participation [`Space`]. An initiative space hosts the
/// signature-gathering and, once escalated, the deliberation around a citizen proposal. It is
/// org-scoped: [`Space::ensure_open`] confirms the hosting organization exists.
#[derive(Clone)]
pub struct InitiativesSpace {
    db: Db,
}

impl std::fmt::Debug for InitiativesSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitiativesSpace")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl InitiativesSpace {
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

impl Space for InitiativesSpace {
    fn kind(&self) -> &'static str {
        SPACE_KIND
    }

    fn allows_component(&self, component: &str) -> bool {
        domain::allows_component(component)
    }

    fn ensure_open(&self, org: OrgId, _space: SpaceId) -> impl Future<Output = Result<()>> + Send {
        // The initiatives registry is hosted per organization; "open" means the org exists. The
        // `SpaceId` is the logical handle for the org's initiatives space (there is no separate
        // spaces table to consult). Read-only and cheap.
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
        // Guards against accidental rename that would break wiring/routing.
        assert_eq!(CRATE_NAME, "dsoc-initiatives");
    }

    #[test]
    fn space_kind_is_initiatives() {
        assert_eq!(SPACE_KIND, "initiatives");
    }
}
