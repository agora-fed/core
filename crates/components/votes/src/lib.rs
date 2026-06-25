//! # dsoc-votes
//!
//! Tier 2 crate. Votes: support signals on proposals. Stores aggregates queryable by officials; individual voter linkage is minimized/protected (LGPD, DO-NOT in PLAN.md).
//!
//! ## Contract
//! - **Emits:** votes.cast, votes.tally.updated
//! - **Consumes:** (none)
//! - **Owns tables:** vote, vote_tally
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the injected event bus / transactional outbox, and the gateway. It never reaches into
//! another crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md). Layering:
//! - [`domain`] — pure value-logic + the privacy-safe aggregate type (no `sqlx`/`axum`).
//! - [`queries`] — every compile-time-checked `sqlx` statement; the protected-linkage write and
//!   the official-facing aggregate reads live in separate functions.
//! - [`service`] — [`VoteService`] over the `Db` pool + injected `Arc<dyn Clock>`; emits via the
//!   transactional outbox (ADR-0006).
//! - [`events`] — emission/consumption over the frozen `dsoc_core::events` catalog.
//! - [`dto`] / [`http`] — the Axum surface mounted by the gateway via [`routes`].

#![forbid(unsafe_code)]

pub mod domain;
pub mod dto;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{TallyView, MIN_VOTE_LEVEL};
pub use dto::{CastVoteRequest, TallyDto, VoteReceiptDto};
pub use http::routes;
pub use service::{CastReceipt, VoteService};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-votes";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-votes");
    }
}
