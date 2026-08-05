//! Event wiring: build the durable envelopes this crate emits through the **transactional outbox**
//! (ADR-0006), and extract the payload of a consumed `proposals.threshold.crossed` envelope. Only the
//! frozen `dsoc_core::events::Event` variants are used (the catalog is frozen — ADR-0004). The
//! extraction half is pure and unit-tested; outbox emission is exercised by the integration tests that
//! read `events_log`.

use chrono::{DateTime, Utc};
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{ClusterId, EventId, MandateId, OrgId, ProposalId};

/// Build a durable [`EventEnvelope`], stamping it with a fresh id, the org, and an explicit time `at`
/// taken from the injected clock (never ambient — TESTING.md). The caller writes it to the outbox
/// (`dsoc_db::outbox::publish_tx`) inside the same transaction as the domain write, so the change and
/// the event commit atomically (ADR-0006).
#[must_use]
pub fn envelope(org: OrgId, at: DateTime<Utc>, event: Event) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event,
    }
}

/// The payload carried by a `proposals.threshold.crossed` event: the demand that crossed its directed
/// support threshold, the consensus cluster it belongs to, and the targeted official's mandate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThresholdCrossed {
    /// The proposal whose support crossed the directed threshold.
    pub proposal: ProposalId,
    /// The consensus cluster the proposal belongs to.
    pub cluster: ClusterId,
    /// The mandate (official/candidate) the demand is directed at.
    pub mandate: MandateId,
}

/// Extract a [`ThresholdCrossed`] from a `proposals.threshold.crossed` envelope, ignoring every other
/// event type. This is the consume-side filter that starts an SLA clock.
#[must_use]
pub fn threshold_crossed(envelope: &EventEnvelope) -> Option<ThresholdCrossed> {
    match envelope.event {
        Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate,
            ..
        } => Some(ThresholdCrossed {
            proposal,
            cluster,
            mandate,
        }),
        _ => None,
    }
}

/// Extract ONE [`ThresholdCrossed`] per recipient (0537 multi-office): the event's
/// `mandates` list expands into per-gabinete crossings sharing the same proposal/cluster.
/// Legacy events (empty list) fall back to the primary `mandate` — exactly the pre-0537
/// behaviour. `None` for any other event type.
#[must_use]
pub fn threshold_crossed_all(envelope: &EventEnvelope) -> Option<Vec<ThresholdCrossed>> {
    match &envelope.event {
        Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate,
            mandates,
        } => {
            let list: Vec<MandateId> = if mandates.is_empty() {
                vec![*mandate]
            } else {
                mandates.clone()
            };
            Some(
                list.into_iter()
                    .map(|m| ThresholdCrossed {
                        proposal: *proposal,
                        cluster: *cluster,
                        mandate: m,
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsoc_core::ids::VoteId;

    fn env(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn extracts_threshold_crossed() {
        let proposal = ProposalId::new();
        let cluster = ClusterId::new();
        let mandate = MandateId::new();
        let e = env(Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate,
            mandates: Vec::new(),
        });
        assert_eq!(
            threshold_crossed(&e),
            Some(ThresholdCrossed {
                proposal,
                cluster,
                mandate
            })
        );
        // Legacy event (empty mandates): the expansion falls back to the primary.
        assert_eq!(
            threshold_crossed_all(&e),
            Some(vec![ThresholdCrossed {
                proposal,
                cluster,
                mandate
            }])
        );
    }

    #[test]
    fn expands_one_crossing_per_gabinete() {
        let proposal = ProposalId::new();
        let cluster = ClusterId::new();
        let m1 = MandateId::new();
        let m2 = MandateId::new();
        let e = env(Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate: m1,
            mandates: vec![m1, m2],
        });
        let all = threshold_crossed_all(&e).expect("threshold event");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].mandate, m1);
        assert_eq!(all[1].mandate, m2);
        assert!(all
            .iter()
            .all(|t| t.proposal == proposal && t.cluster == cluster));
    }

    #[test]
    fn ignores_other_events() {
        let e = env(Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        });
        assert_eq!(threshold_crossed(&e), None);
    }

    #[test]
    fn envelope_stamps_org_and_time() {
        let org = OrgId::new();
        let at = Utc::now();
        let e = envelope(
            org,
            at,
            Event::ConsequenceSlaExpired {
                sla: dsoc_core::ids::SlaId::new(),
                mandate: MandateId::new(),
            },
        );
        assert_eq!(e.org, org);
        assert_eq!(e.at, at);
    }
}
