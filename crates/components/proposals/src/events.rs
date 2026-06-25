//! Event wiring: build the durable envelope proposals emits through the **transactional outbox**
//! (ADR-0006), and extract the payloads carried by the envelopes this crate consumes. Only existing
//! `dsoc_core::events::Event` variants are used (the catalog is frozen — ADR-0004). The extraction
//! half is pure and unit-tested; outbox emission is exercised by the integration tests that read
//! `events_log`.

use std::sync::Arc;

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{ClusterId, EventId, OrgId, ProposalId};
use dsoc_core::Clock;

/// Build a durable [`EventEnvelope`], stamping it with a fresh id, the org, and the injected clock's
/// time (never ambient — TESTING.md). The caller writes it to the outbox
/// (`dsoc_db::outbox::publish_tx`) inside the same transaction as the domain write, so the change and
/// the event commit atomically (ADR-0006).
#[must_use]
pub fn envelope(clock: &Arc<dyn Clock>, org: OrgId, event: Event) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event,
    }
}

/// Extract `(proposal, cluster)` from a `consensus.proposal.merged` envelope — the linking signal
/// that folds a proposal into a consensus cluster. Every other event type yields `None`.
///
/// Note: `consensus.cluster.formed` carries only `(cluster, size)` — no proposal id — so it cannot
/// link a specific proposal; the merge event is the actionable linking signal (see [`linkable`]).
#[must_use]
pub fn consensus_proposal_merged(envelope: &EventEnvelope) -> Option<(ProposalId, ClusterId)> {
    match envelope.event {
        Event::ConsensusProposalMerged { proposal, cluster } => Some((proposal, cluster)),
        _ => None,
    }
}

/// Extract the proposal from a `moderation.cleared` envelope — the gate that allows publication.
#[must_use]
pub fn moderation_cleared(envelope: &EventEnvelope) -> Option<ProposalId> {
    match envelope.event {
        Event::ModerationCleared { proposal } => Some(proposal),
        _ => None,
    }
}

/// Extract `(proposal, support_count)` from a `votes.tally.updated` envelope — the privacy-safe
/// aggregate this crate folds into its own `support_count` (it never reads the votes tables).
#[must_use]
pub fn vote_tally_updated(envelope: &EventEnvelope) -> Option<(ProposalId, u64)> {
    match envelope.event {
        Event::VoteTallyUpdated {
            proposal,
            support_count,
        } => Some((proposal, support_count)),
        _ => None,
    }
}

/// Whether an inbound envelope is one this crate links/clusters a proposal from. Used to recognise
/// `consensus.*` envelopes; `consensus.cluster.formed` is recognised but not linkable on its own.
#[must_use]
pub fn linkable(envelope: &EventEnvelope) -> bool {
    matches!(
        envelope.event,
        Event::ConsensusProposalMerged { .. } | Event::ConsensusClusterFormed { .. }
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use dsoc_core::ids::{MandateId, VoteId};

    fn env(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn extracts_consensus_proposal_merged() {
        let proposal = ProposalId::new();
        let cluster = ClusterId::new();
        let e = env(Event::ConsensusProposalMerged { proposal, cluster });
        assert_eq!(consensus_proposal_merged(&e), Some((proposal, cluster)));
    }

    #[test]
    fn cluster_formed_is_not_a_merge_link() {
        let e = env(Event::ConsensusClusterFormed {
            cluster: ClusterId::new(),
            size: 3,
        });
        assert_eq!(consensus_proposal_merged(&e), None);
        assert!(
            linkable(&e),
            "cluster.formed is still a recognised consensus event"
        );
    }

    #[test]
    fn extracts_moderation_cleared() {
        let proposal = ProposalId::new();
        let e = env(Event::ModerationCleared { proposal });
        assert_eq!(moderation_cleared(&e), Some(proposal));
    }

    #[test]
    fn extracts_vote_tally_updated() {
        let proposal = ProposalId::new();
        let e = env(Event::VoteTallyUpdated {
            proposal,
            support_count: 42,
        });
        assert_eq!(vote_tally_updated(&e), Some((proposal, 42)));
    }

    #[test]
    fn ignores_unrelated_events() {
        let e = env(Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        });
        assert_eq!(consensus_proposal_merged(&e), None);
        assert_eq!(moderation_cleared(&e), None);
        assert_eq!(vote_tally_updated(&e), None);
        assert!(!linkable(&e));
    }

    #[test]
    fn envelope_stamps_clock_time() {
        struct FixedClock(chrono::DateTime<Utc>);
        impl Clock for FixedClock {
            fn now(&self) -> chrono::DateTime<Utc> {
                self.0
            }
        }
        let t = Utc::now();
        let clock: Arc<dyn Clock> = Arc::new(FixedClock(t));
        let org = OrgId::new();
        let e = envelope(
            &clock,
            org,
            Event::ProposalCreated {
                proposal: ProposalId::new(),
                mandate: MandateId::new(),
            },
        );
        assert_eq!(e.at, t);
        assert_eq!(e.org, org);
    }
}
