//! Shared data-transfer objects. These are the *public* shapes; internal domain types
//! live in their owning crates and are mapped to these at the gateway boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Public view of a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProposalDto {
    /// Proposal id.
    pub id: Uuid,
    /// Title (Portuguese, civic content).
    pub title: String,
    /// Body / description.
    pub body: String,
    /// The mandate this proposal is directed at.
    pub mandate_id: Uuid,
    /// Consensus cluster this proposal was merged into, if any.
    pub cluster_id: Option<Uuid>,
    /// Aggregate support count (never per-citizen linkage).
    pub support_count: u64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Public view of a mandate / candidacy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MandateDto {
    /// Mandate id.
    pub id: Uuid,
    /// Office held or sought.
    pub office: String,
    /// Public display name.
    pub display_name: String,
    /// Whether this is a candidacy (vs sitting office).
    pub is_candidate: bool,
    /// Whether the official has completed onboarding.
    pub onboarded: bool,
}

/// State of a consequence SLA, surfaced to clients (the emotional core of the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlaStatus {
    /// Clock running; official notified, awaiting response.
    Pending,
    /// Official responded in time.
    Answered,
    /// Official acted (response plus a concrete commitment).
    Acted,
    /// SLA expired with no response — public silence.
    Ignored,
}

/// Public per-politician scorecard summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScorecardDto {
    /// Mandate this scorecard belongs to.
    pub mandate_id: Uuid,
    /// Proposals answered within SLA.
    pub answered: u64,
    /// Proposals ignored (public silence).
    pub ignored: u64,
    /// Median response latency in hours (None if no responses yet).
    pub median_response_hours: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sla_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&SlaStatus::Ignored).unwrap(),
            "\"ignored\""
        );
    }
}
