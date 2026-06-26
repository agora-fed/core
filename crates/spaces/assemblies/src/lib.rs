//! # dsoc-assemblies
//!
//! Tier 2 crate. Assemblies: permanent participatory bodies with membership. A citizen joins a
//! standing body (an `assembly`) with a role; the body hosts the recurring deliberation loop
//! (proposals, votes, comments, meetings) over time.
//!
//! ## Contract
//! - **Emits:** (none) — membership is this crate's internal state; it publishes no cross-crate
//!   events (the `assemblies.*` topics in the original sketch are not in the frozen
//!   `dsoc_core::events` catalog and nothing consumes them, so none are emitted).
//! - **Consumes:** (none).
//! - **Owns tables:** `assembly`, `assembly_member` (migration `0220_assemblies_membership.sql`).
//!
//! ## Layering
//! - [`domain`] — pure value logic (text/role validation, component gating), unit-tested.
//! - [`queries`] — every compile-time-checked `sqlx` statement (private).
//! - [`service`] — [`AssembliesService`]: `Db` + injected `Clock`/`Authorization`.
//! - [`http`] — the Axum surface mounted by the gateway via [`routes`].
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits and the gateway. It
//! never reaches into a peer crate's internals (PLAN.md / CONTRIBUTING.md), and depends only on
//! Tier-0 (`core`/`db`/`api-contract`/`app`).

#![forbid(unsafe_code)]

pub mod domain;
pub mod http;
mod queries;
pub mod service;

use std::future::Future;

use dsoc_app::AppState;
use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Error, Result};
use dsoc_db::Db;

pub use http::routes;
pub use service::{AssembliesService, Assembly, Member, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-assemblies";

/// The stable machine name of this participation space (the [`Space::kind`] value).
pub const SPACE_KIND: &str = "assemblies";

/// The assemblies registry as a participation [`Space`]. Each assembly is a standing body whose id
/// doubles as its [`SpaceId`]; [`Space::ensure_open`] confirms that specific assembly exists in the
/// organization.
#[derive(Clone)]
pub struct AssembliesSpace {
    db: Db,
}

impl std::fmt::Debug for AssembliesSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssembliesSpace")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl AssembliesSpace {
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

impl Space for AssembliesSpace {
    fn kind(&self) -> &'static str {
        SPACE_KIND
    }

    fn allows_component(&self, component: &str) -> bool {
        domain::allows_component(component)
    }

    fn ensure_open(&self, org: OrgId, space: SpaceId) -> impl Future<Output = Result<()>> + Send {
        // An assembly's id IS the SpaceId; "open" means that assembly exists in the organization.
        let db = self.db.clone();
        async move {
            let exists = queries::assembly_exists(&db, org.as_uuid(), space.as_uuid())
                .await
                .map_err(|e| Error::Storage(Box::new(e)))?;
            if exists {
                Ok(())
            } else {
                Err(Error::NotFound("assembly not found".to_owned()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break wiring.
        assert_eq!(CRATE_NAME, "dsoc-assemblies");
    }

    #[test]
    fn space_kind_is_assemblies() {
        assert_eq!(SPACE_KIND, "assemblies");
    }
}
