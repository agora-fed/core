//! # dsoc-processes
//!
//! Tier 2 crate. Participatory processes: time-bound civic spaces grouping components toward a goal
//! (the Decidim "Participatory Process" concept, re-architected — credited in CHANGELOG). A process
//! groups participation components (proposals, votes, comments, debates, …) and walks them through
//! ordered, advanceable phases between a start and an end.
//!
//! ## Contract
//! - **Emits:** none. The frozen `dsoc_core::events` catalog has no `processes.*` variants, and
//!   adding one is a Tier-0 change (an ADR). Like `dsoc-admin`, this crate persists state and
//!   exposes routes but emits no cross-crate events for now.
//! - **Consumes:** none.
//! - **Owns tables:** `process`, `process_phase` (migration `0210_processes_core.sql`).
//!
//! ## Layering
//! - [`domain`] — pure value logic (status, validation, authorization gating, phase rule), unit-tested.
//! - `queries` — every compile-time-checked `sqlx` statement (private).
//! - [`service`] — [`ProcessService`]: `Db` + injected `Clock`/`Authorization`.
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

pub use domain::ProcessStatus;
pub use http::routes;
pub use service::{
    Phase, Process, ProcessDraft, ProcessService, ProcessUpdate, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-processes";

/// The stable machine name of this participation space (the [`Space::kind`] value).
pub const SPACE_KIND: &str = "processes";

/// A participatory process as a [`Space`]. A process space hosts the deliberative loop
/// (proposals, votes, comments, debates, …) between a start and an end. It is org-scoped, and a
/// given [`SpaceId`] is the id of the `process` row it represents; [`Space::ensure_open`] confirms
/// that process exists and is currently `active`.
#[derive(Clone)]
pub struct ProcessesSpace {
    db: Db,
}

impl std::fmt::Debug for ProcessesSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessesSpace")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl ProcessesSpace {
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

impl Space for ProcessesSpace {
    fn kind(&self) -> &'static str {
        SPACE_KIND
    }

    fn allows_component(&self, component: &str) -> bool {
        domain::allows_component(component)
    }

    fn ensure_open(&self, org: OrgId, space: SpaceId) -> impl Future<Output = Result<()>> + Send {
        // A process space is "open" when its `process` row exists in the org and is `active`. The
        // `SpaceId` is the id of that process row. Read-only and cheap.
        let db = self.db.clone();
        async move {
            let row = queries::get_process(&db, org.as_uuid(), space.as_uuid())
                .await
                .map_err(|e| Error::Storage(Box::new(e)))?
                .ok_or_else(|| Error::NotFound("process not found".to_owned()))?;
            if row.status == ProcessStatus::Active.as_str() {
                Ok(())
            } else {
                Err(Error::Conflict(
                    "process is not open for participation".to_owned(),
                ))
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
        assert_eq!(CRATE_NAME, "dsoc-processes");
    }

    #[test]
    fn space_kind_is_processes() {
        assert_eq!(SPACE_KIND, "processes");
    }
}
