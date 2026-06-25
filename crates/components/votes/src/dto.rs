//! The crate's own request/response DTOs plus their `utoipa` schema fragment (ADR-0004:
//! `api-contract` holds only the envelope/error/pagination; each crate owns its shapes and the
//! gateway composes `/openapi.json`). Domain types are mapped to these at the HTTP boundary.
//!
//! The official-facing [`TallyDto`] has **no citizen field** — the LGPD privacy invariant is part
//! of the public contract, not just an implementation detail.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::TallyView;
use crate::service::CastReceipt;

/// `POST /votes` body: a citizen casts a support signal for a proposal. `citizen_id` is the
/// authenticated caller; the handler authorizes it against the org (email-verified minimum) before
/// any write — the id is never trusted blind.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CastVoteRequest {
    /// Organization/tenant the vote belongs to.
    pub org_id: Uuid,
    /// The proposal being supported.
    pub proposal_id: Uuid,
    /// The authenticated citizen casting the support signal.
    pub citizen_id: Uuid,
}

/// `POST /votes` response: the voter's own receipt. Returned only to the caster, it confirms their
/// vote id and the proposal's new aggregate count. It deliberately echoes no other citizen's data.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VoteReceiptDto {
    /// The opaque id of the recorded vote.
    pub vote_id: Uuid,
    /// The proposal supported.
    pub proposal_id: Uuid,
    /// The proposal's new aggregate support count.
    pub support_count: u64,
}

impl From<CastReceipt> for VoteReceiptDto {
    fn from(receipt: CastReceipt) -> Self {
        Self {
            vote_id: receipt.vote.as_uuid(),
            proposal_id: receipt.proposal.as_uuid(),
            support_count: receipt.support_count,
        }
    }
}

/// Official-facing aggregate view of a proposal's support. Carries the count only — **never** any
/// citizen linkage (LGPD). This shape is the only vote data an official may read.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct TallyDto {
    /// The proposal the aggregate is for.
    pub proposal_id: Uuid,
    /// Distinct supporters — a count only.
    pub support_count: u64,
    /// When the aggregate last changed.
    pub updated_at: DateTime<Utc>,
}

impl From<TallyView> for TallyDto {
    fn from(view: TallyView) -> Self {
        Self {
            proposal_id: view.proposal.as_uuid(),
            support_count: view.support_count,
            updated_at: view.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use dsoc_core::ids::{ProposalId, VoteId};

    #[test]
    fn tally_dto_provably_carries_no_citizen_id() {
        // Build the official-facing DTO from a domain view and serialize it. The wire form must
        // not contain any citizen linkage — the privacy invariant proven over the actual bytes a
        // client would receive.
        let view = TallyView {
            proposal: ProposalId::new(),
            support_count: 5,
            updated_at: Utc::now(),
        };
        let dto = TallyDto::from(view);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("citizen"));
        assert!(json.contains("support_count"));
        assert!(json.contains("proposal_id"));
        // Exactly the three sanctioned fields, nothing more.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert!(obj.contains_key("proposal_id"));
        assert!(obj.contains_key("support_count"));
        assert!(obj.contains_key("updated_at"));
    }

    #[test]
    fn receipt_dto_maps_ids_and_count() {
        let vote = VoteId::new();
        let proposal = ProposalId::new();
        let dto = VoteReceiptDto::from(CastReceipt {
            vote,
            proposal,
            support_count: 9,
        });
        assert_eq!(dto.vote_id, vote.as_uuid());
        assert_eq!(dto.proposal_id, proposal.as_uuid());
        assert_eq!(dto.support_count, 9);
    }
}
