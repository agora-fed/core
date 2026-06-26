//! # dsoc-consultations
//!
//! Tier 2 crate. Consultations: scoped questions put to the population with defined response windows.
//!
//! ## Contract
//! - **Emits:** (none — see Events note below)
//! - **Consumes:** (none)
//! - **Owns tables:** `consultations_consultation`, `consultations_consultation_question`
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the `db` pool, the `app` wiring, and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md). Layering:
//! - [`domain`] — pure status/validation value-logic + view types (no `sqlx`/`axum`).
//! - [`queries`] — every compile-time-checked `sqlx` statement; no `SELECT *`, keyset lists.
//! - [`service`] — [`ConsultationService`] over the `Db` pool + injected `Arc<dyn Clock>`.
//! - [`space`] — the [`dsoc_core::traits::Space`] impl for kind `"consultations"`.
//! - [`dto`] / [`http`] — the Axum surface mounted by the gateway via [`routes`].
//!
//! ## Events
//! This crate emits **no** cross-crate events: the topics named in older drafts
//! (`consultations.*`) are not part of the frozen `dsoc_core::events` catalog and nothing
//! consumes them, so — like `dsoc-admin` — it keeps its effects local to its own tables.

#![forbid(unsafe_code)]

pub mod domain;
pub mod dto;
pub mod http;
pub mod queries;
pub mod service;
pub mod space;

pub use domain::{ConsultationStatus, ConsultationView, QuestionView, MIN_MANAGE_LEVEL};
pub use dto::{ConsultationDto, CreateConsultationRequest, QuestionDto};
pub use http::routes;
pub use service::ConsultationService;
pub use space::ConsultationSpace;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-consultations";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-consultations");
    }
}
