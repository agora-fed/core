//! # dsoc-proposals
//!
//! Tier 2 crate. Proposals: the primary civic artifact citizens create, directed at a mandate or
//! campaign. This crate owns the **threshold logic that triggers the consequence loop** — the
//! accountability thesis: many near-duplicate proposals are folded by `consensus` into one cluster,
//! and when that clustered signal's aggregate support crosses the directed threshold, exactly one
//! `proposals.threshold.crossed` fires (never re-fired by duplicate vote signals).
//!
//! ## Contract
//! - **Emits:** proposals.created, proposals.published, proposals.threshold.crossed
//! - **Consumes:** consensus.cluster.formed, consensus.proposal.merged, moderation.cleared,
//!   votes.tally.updated
//! - **Owns tables:** proposal, proposal_revision
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, the event bus
//! (`dsoc-events`), and the gateway. It never reaches into another crate's internals (see `DO NOT`
//! in PLAN.md and CONTRIBUTING.md). Events are emitted through the **transactional outbox**
//! (ADR-0006) so a domain change and its event commit atomically.

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{crossing_fires, merge_support, NewProposal, NewRevision, ProposalStatus};
pub use http::routes;
pub use queries::{ProposalRow, RevisionRow};
pub use service::{Consumed, ProposalService};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-proposals";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-proposals");
    }
}
