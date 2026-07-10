//! # dsoc-consensus
//!
//! Tier 1 crate. Semantic clustering & dedupe via pgvector. Embeds every new proposal, finds near-duplicates, and merges noise into one consensus signal (Decidim failure #2).
//!
//! ## Contract
//! - **Emits:** consensus.cluster.formed, consensus.proposal.merged
//! - **Consumes:** proposals.created
//! - **Owns tables:** consensus_embedding, consensus_cluster, consensus_cluster_member
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
#[cfg(feature = "model-embedder")]
pub mod model_embedder;
pub mod queries;
pub mod service;

pub use domain::{cosine_distance, Decision, Embedder, Placement, StubEmbedder, DEFAULT_THRESHOLD};
pub use http::routes;
#[cfg(feature = "model-embedder")]
pub use model_embedder::ModelEmbedder;
pub use service::ClusterService;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-consensus";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-consensus");
    }
}
