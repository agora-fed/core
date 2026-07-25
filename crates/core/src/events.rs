//! The **frozen event catalog** — the only sanctioned cross-crate contract besides
//! `core` traits and the gateway (PLAN.md sections 5.2, 6.2). A crate emits events and
//! subscribes to events; it never calls a peer crate directly.
//!
//! Adding a variant is backward-compatible; changing or removing one requires an ADR.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{
    CitizenId, ClusterId, CommentId, EventId, MandateId, NotificationId, OrgId, ProposalId,
    ScorecardId, SlaId, VoteId,
};

/// Stable topic strings used for routing and durable subscriptions (pgmq queue names).
/// These strings are part of the frozen contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTopic {
    /// `proposals.*`
    Proposals,
    /// `votes.*`
    Votes,
    /// `comments.*`
    Comments,
    /// `consensus.*`
    Consensus,
    /// `consequence.*`
    Consequence,
    /// `mandates.*`
    Mandates,
    /// `scorecard.*`
    Scorecard,
    /// `moderation.*`
    Moderation,
    /// `auth.*`
    Auth,
    /// `notify.*`
    Notify,
}

impl EventTopic {
    /// The routing key prefix for this topic.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EventTopic::Proposals => "proposals",
            EventTopic::Votes => "votes",
            EventTopic::Comments => "comments",
            EventTopic::Consensus => "consensus",
            EventTopic::Consequence => "consequence",
            EventTopic::Mandates => "mandates",
            EventTopic::Scorecard => "scorecard",
            EventTopic::Moderation => "moderation",
            EventTopic::Auth => "auth",
            EventTopic::Notify => "notify",
        }
    }
}

/// The catalog of every cross-crate event. Tagged for stable JSON on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum Event {
    // --- proposals ---
    /// A citizen created a proposal directed at a mandate/campaign.
    #[serde(rename = "proposals.created")]
    ProposalCreated {
        proposal: ProposalId,
        mandate: MandateId,
    },
    /// A proposal's support crossed its directed threshold.
    #[serde(rename = "proposals.threshold.crossed")]
    ProposalThresholdCrossed {
        proposal: ProposalId,
        cluster: ClusterId,
        /// The PRINCIPAL target (kept for wire/back-compat with events already in the log).
        mandate: MandateId,
        /// TODOS os destinatários da proposta, principal incluído (0537 multi-gabinete).
        /// Vazio em eventos antigos — consumidores caem pra `vec![mandate]`.
        #[serde(default)]
        mandates: Vec<MandateId>,
    },

    // --- votes ---
    /// A support vote was cast (aggregate signal; voter linkage stays protected).
    #[serde(rename = "votes.cast")]
    VoteCast { vote: VoteId, proposal: ProposalId },

    // --- comments ---
    /// A comment was added to a deliberation thread.
    #[serde(rename = "comments.created")]
    CommentCreated {
        comment: CommentId,
        proposal: ProposalId,
    },

    // --- consensus ---
    /// `consensus` merged near-duplicates into one cluster signal.
    #[serde(rename = "consensus.cluster.formed")]
    ConsensusClusterFormed { cluster: ClusterId, size: u32 },

    // --- consequence (the thesis) ---
    /// An SLA clock started against an official for a clustered proposal.
    #[serde(rename = "consequence.sla.started")]
    ConsequenceSlaStarted {
        sla: SlaId,
        mandate: MandateId,
        cluster: ClusterId,
        due: DateTime<Utc>,
    },
    /// The SLA expired with no response — public silence recorded.
    #[serde(rename = "consequence.sla.expired")]
    ConsequenceSlaExpired { sla: SlaId, mandate: MandateId },
    /// The official responded within the SLA window.
    #[serde(rename = "consequence.official.responded")]
    ConsequenceOfficialResponded { sla: SlaId, mandate: MandateId },

    // --- mandates ---
    /// An official/candidate was invited to onboard via their public email.
    #[serde(rename = "mandates.official.invited")]
    MandateOfficialInvited { mandate: MandateId },
    /// An official completed onboarding.
    #[serde(rename = "mandates.official.onboarded")]
    MandateOfficialOnboarded { mandate: MandateId },

    // --- scorecard ---
    /// A politician's public scorecard changed.
    #[serde(rename = "scorecard.updated")]
    ScorecardUpdated {
        scorecard: ScorecardId,
        mandate: MandateId,
    },

    // --- moderation ---
    /// Content was flagged by rules/statistics/local model (auditable).
    #[serde(rename = "moderation.flagged")]
    ModerationFlagged { proposal: ProposalId },
    /// Content cleared moderation.
    #[serde(rename = "moderation.cleared")]
    ModerationCleared { proposal: ProposalId },

    // --- additive (ADR-0004): seams the component CRATE.md contracts reference ---
    /// A proposal cleared moderation and was published.
    #[serde(rename = "proposals.published")]
    ProposalPublished {
        proposal: ProposalId,
        mandate: MandateId,
    },
    /// A proposal's aggregate support tally changed (privacy-safe aggregate only).
    #[serde(rename = "votes.tally.updated")]
    VoteTallyUpdated {
        proposal: ProposalId,
        support_count: u64,
    },
    /// `consensus` merged a proposal into an existing cluster.
    #[serde(rename = "consensus.proposal.merged")]
    ConsensusProposalMerged {
        proposal: ProposalId,
        cluster: ClusterId,
    },
    /// A mandate's identity was verified to a higher assurance level.
    #[serde(rename = "mandates.identity.verified")]
    MandateIdentityVerified { mandate: MandateId },
    /// A citizen's verification level was upgraded by `auth`.
    #[serde(rename = "auth.verification.upgraded")]
    AuthVerificationUpgraded { citizen: CitizenId },
    /// A notification was dispatched to a channel.
    #[serde(rename = "notify.dispatched")]
    NotifyDispatched { notification: NotificationId },
    /// A notification failed delivery (after retries).
    #[serde(rename = "notify.delivery.failed")]
    NotifyDeliveryFailed { notification: NotificationId },
}

impl Event {
    /// The topic this event routes under.
    #[must_use]
    pub const fn topic(&self) -> EventTopic {
        match self {
            Event::ProposalCreated { .. } | Event::ProposalThresholdCrossed { .. } => {
                EventTopic::Proposals
            }
            Event::VoteCast { .. } => EventTopic::Votes,
            Event::CommentCreated { .. } => EventTopic::Comments,
            Event::ConsensusClusterFormed { .. } => EventTopic::Consensus,
            Event::ConsequenceSlaStarted { .. }
            | Event::ConsequenceSlaExpired { .. }
            | Event::ConsequenceOfficialResponded { .. } => EventTopic::Consequence,
            Event::MandateOfficialInvited { .. } | Event::MandateOfficialOnboarded { .. } => {
                EventTopic::Mandates
            }
            Event::ScorecardUpdated { .. } => EventTopic::Scorecard,
            Event::ModerationFlagged { .. } | Event::ModerationCleared { .. } => {
                EventTopic::Moderation
            }
            Event::ProposalPublished { .. } => EventTopic::Proposals,
            Event::VoteTallyUpdated { .. } => EventTopic::Votes,
            Event::ConsensusProposalMerged { .. } => EventTopic::Consensus,
            Event::MandateIdentityVerified { .. } => EventTopic::Mandates,
            Event::AuthVerificationUpgraded { .. } => EventTopic::Auth,
            Event::NotifyDispatched { .. } | Event::NotifyDeliveryFailed { .. } => {
                EventTopic::Notify
            }
        }
    }
}

/// The durable envelope written to the event log (`dsoc-events`). Carries provenance for
/// the audit trail a politically-targeted platform requires (CICD.md "provenance").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique id of this delivery.
    pub id: EventId,
    /// Originating organization/tenant.
    pub org: OrgId,
    /// When the event occurred (from the injected [`crate::Clock`], never ambient).
    pub at: DateTime<Utc>,
    /// The payload.
    pub event: Event,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_stable_tags() {
        let e = Event::ProposalCreated {
            proposal: ProposalId::new(),
            mandate: MandateId::new(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"proposals.created\""));
        assert_eq!(e.topic(), EventTopic::Proposals);
    }

    #[test]
    fn sla_expiry_routes_to_consequence() {
        let e = Event::ConsequenceSlaExpired {
            sla: SlaId::new(),
            mandate: MandateId::new(),
        };
        assert_eq!(e.topic().as_str(), "consequence");
    }

    #[test]
    fn envelope_roundtrips() {
        let env = EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event: Event::VoteCast {
                vote: VoteId::new(),
                proposal: ProposalId::new(),
            },
        };
        let back: EventEnvelope =
            serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(env, back);
    }
}
