//! # dsoc-app — the wiring contract
//!
//! Holds [`AppState`], the single value every crate's `pub fn routes(state: AppState) -> Router<()>`
//! receives (frozen in ADR-0004). It bridges the `core` service ports and the `db` pool so component
//! crates depend only on Tier-0 (`core`, `db`, `api-contract`, `app`) and never on each other.
//!
//! `AppState` cannot live in `dsoc-core` because it carries the concrete [`dsoc_db::Db`] pool, and
//! `core` must not depend on `db` (the dependency points the other way).

#![forbid(unsafe_code)]

pub mod caller;
pub mod manifest;
pub use caller::CallerId;

use std::sync::Arc;

use dsoc_core::{Authorization, Clock, EventBus, Storage};
use dsoc_db::Db;

/// Shared, cheaply-cloneable application state injected into every crate's router.
#[derive(Clone)]
pub struct AppState {
    /// The PostgreSQL connection pool.
    pub db: Db,
    /// The durable event publish port (concrete impl from `dsoc-events`).
    pub bus: Arc<dyn EventBus>,
    /// The authorization/verification port (concrete impl from `dsoc-auth`).
    pub authz: Arc<dyn Authorization>,
    /// The injected clock (never read time ambiently — TESTING.md).
    pub clock: Arc<dyn Clock>,
    /// Blob storage port (ADR-0010 W1.2). `None` when no `STORAGE_*` env is configured — the
    /// gateway then returns 503 on upload endpoints and avatar URLs render as `None` everywhere.
    pub storage: Option<Arc<dyn Storage>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}
