//! The service traits every Tier-1+ crate implements or consumes. These are the
//! synchronous interface half of the contract (the asynchronous half is [`crate::events`]).
//!
//! `async fn` in traits is used directly (stable since Rust 1.75). Crates depend on these
//! traits — and on test mocks of them — never on a peer crate's concrete type.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::events::EventEnvelope;
use crate::ids::{CitizenId, MandateId, OrgId, SpaceId};

/// Identity assurance level for a participant (PLAN.md `mandates`: email-only → TSE-backed).
/// Ordered: higher levels unlock higher-trust actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLevel {
    /// Anonymous / unverified visitor.
    Anonymous,
    /// Email-confirmed citizen.
    Email,
    /// Verified against an official public directory (e.g. invited official).
    Directory,
    /// Strongly verified (e.g. TSE-backed). Reserved for later phases.
    Strong,
}

/// Authorization: "is this person who they claim, and may they act?" Implemented by `auth`.
/// `async-trait` keeps it dyn-compatible so it can be injected as `Arc<dyn Authorization>`.
#[async_trait::async_trait]
pub trait Authorization: Send + Sync {
    /// Resolve the verification level of a citizen within an organization.
    async fn level(&self, org: OrgId, citizen: CitizenId) -> Result<VerificationLevel>;

    /// Assert the citizen meets at least `required`, else [`crate::Error::Forbidden`].
    async fn require(
        &self,
        org: OrgId,
        citizen: CitizenId,
        required: VerificationLevel,
    ) -> Result<()>;
}

/// The publish port for the durable event bus. A crate emits cross-crate signals through this
/// (injected as `Arc<dyn EventBus>`) instead of depending on `dsoc-events` — preserving the crate
/// boundary (PLAN.md section 5.2, ADR-0004). The concrete implementation lives in `dsoc-events`.
#[async_trait::async_trait]
pub trait EventBus: Send + Sync {
    /// Durably publish an event envelope. Implementations must be at-least-once; consumers must be
    /// idempotent.
    async fn publish(&self, envelope: EventEnvelope) -> Result<()>;
}

/// A participation **space** (process, assembly, initiative, consultation, mandate).
/// Implemented by each crate under `crates/spaces`.
pub trait Space: Send + Sync {
    /// Stable machine name (e.g. `"processes"`).
    fn kind(&self) -> &'static str;

    /// Whether `component` may be mounted into this space.
    fn allows_component(&self, component: &str) -> bool;

    /// Confirm a space exists and is open for participation.
    fn ensure_open(
        &self,
        org: OrgId,
        space: SpaceId,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

/// A participation **component** (proposals, votes, comments, …) mounted into a space.
/// Implemented by each crate under `crates/components`.
pub trait Component: Send + Sync {
    /// Stable machine name (e.g. `"proposals"`).
    fn kind(&self) -> &'static str;

    /// The mandate this component instance is directed at, if any (drives the consequence loop).
    fn directed_mandate(&self) -> Option<MandateId>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_levels_are_ordered() {
        assert!(VerificationLevel::Anonymous < VerificationLevel::Email);
        assert!(VerificationLevel::Directory < VerificationLevel::Strong);
    }

    #[test]
    fn verification_level_serializes_snake_case() {
        let json = serde_json::to_string(&VerificationLevel::Directory).unwrap();
        assert_eq!(json, "\"directory\"");
    }
}
