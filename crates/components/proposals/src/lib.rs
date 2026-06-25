//! # dsoc-proposals
//!
//! Tier 2 crate. Proposals: the primary civic artifact citizens create, directed at a mandate or campaign.
//!
//! ## Contract
//! - **Emits:** proposals.created, proposals.published, proposals.threshold.crossed
//! - **Consumes:** consensus.cluster.formed, moderation.cleared
//! - **Owns tables:** proposal, proposal_revision
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

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
