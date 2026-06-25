//! # dsoc-consequence
//!
//! Tier 2 crate. NEW. Response-or-public-silence engine. When a clustered proposal crosses a support threshold aimed at an official, it starts an SLA clock, notifies them, and on expiry publicly records answered/acted/ignored (Decidim failure #1). This is the point of the platform.
//!
//! ## Contract
//! - **Emits:** consequence.sla.started, consequence.sla.expired, consequence.official.responded
//! - **Consumes:** proposals.threshold.crossed, consensus.cluster.formed
//! - **Owns tables:** consequence_sla, consequence_response, consequence_outcome
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-consequence";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-consequence");
    }
}
