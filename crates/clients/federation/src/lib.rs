//! # dsoc-federation — Tier-3 ActivityPub federation foundation (ADR-0005)
//!
//! A **foundation**, not a full network node: the ActivityPub data model, a namespaced
//! accountability vocabulary, and a read-only discovery surface, so a municipality can later run a
//! local instance that federates its signal into the central platform and renders the consequence
//! loop. Per the Tier-3 rule (PLAN.md §6.1), this crate depends **only on the frozen
//! `dsoc-api-contract`** — never on `core`/`db`/`app` server implementations — so it can be built
//! against mocks and stays a pure client of the published shapes.
//!
//! ## Layering
//! - [`vocab`] — JSON-LD `@context` and the `dsoc:` accountability namespace.
//! - [`actor`] — the AP `Actor` (`Person`) and the voter -> candidate -> official identity
//!   progression (one stable identity the scorecard follows).
//! - [`objects`] — AP `Note`/`Create` plus the accountability objects (`Proposal`,
//!   `ConsensusCluster`, `Sla`, `Scorecard`, `SupportTally`).
//! - [`mapping`] — `api-contract` DTOs -> AP objects; **support stays an aggregate, never a
//!   per-actor public `Like`** (vote privacy / LGPD).
//! - [`webfinger`] / [`nodeinfo`] — discovery builders (pure `serde_json::Value`).
//! - [`http`] — the read-only Axum surface (`routes()`), mounted by the gateway (no DB, no bind).
//! - [`signatures`] — HTTP Signatures types + a verify-interface trait + the pure signing-string
//!   builder (no live network delivery).
//!
//! ## Contract
//! - **Emits:** `federation.instance.registered`, `federation.signal.synced` (Phase 3).
//! - **Consumes:** (none)
//! - **Owns tables:** `federation_instance`, `federation_sync_cursor` (Phase 3).

#![forbid(unsafe_code)]

pub mod actor;
pub mod http;
pub mod mapping;
pub mod nodeinfo;
pub mod objects;
pub mod signatures;
pub mod vocab;
pub mod webfinger;

// Re-export the most-used surface so consumers can `use dsoc_federation::{Actor, ActorRole, ...}`.
pub use actor::{
    actor_id, citizen_handle, derive_actor, derive_actor_with_role, derive_citizen_actor,
    mandate_handle, Actor, ActorRole, ActorType,
};
pub use http::routes;
pub use mapping::{
    cluster_to_ap, mandate_to_actor, proposal_to_ap, scorecard_to_ap, sla_to_ap, support_tally,
};
pub use objects::{ConsensusCluster, Create, Note, Proposal, Scorecard, Sla, SupportTally};
pub use signatures::{
    build_signing_string, PublicKey, SignatureAlgorithm, SignatureHeader, SignatureVerifier,
};
pub use vocab::{Context, ACCOUNTABILITY_NS, ACTIVITYSTREAMS_NS, SOFTWARE_NAME};
pub use webfinger::{webfinger_jrd, WebFingerError};

/// Compile-time marker proving the crate name is wired into the workspace (event-routing guard).
pub const CRATE_NAME: &str = "dsoc-federation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-federation");
    }

    #[test]
    fn public_surface_is_reachable() {
        // A smoke test that the re-exported foundation composes end to end.
        let actor = derive_citizen_actor(uuid::Uuid::nil(), "[2001:db8::1]");
        assert_eq!(actor.role, Some(ActorRole::Voter));
        assert!(webfinger_jrd("acct:citizen-x@[2001:db8::1]", "[2001:db8::1]").is_ok());
    }
}
