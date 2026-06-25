//! Cross-crate event emission and consumption. The crate only ever touches the **frozen**
//! `dsoc_core::events` catalog and emits through the **transactional outbox** (ADR-0006) — it
//! never depends on `dsoc-events` (ADR-0004 wiring conventions). Both envelope builders stamp the
//! time from the injected [`Clock`], never the ambient wall clock (TESTING.md).

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, OrgId, ProposalId, VoteId};
use dsoc_core::{Clock, Result};

/// Build the `votes.cast` envelope for a freshly recorded support signal. The payload carries only
/// the (opaque) vote id and the proposal — never the citizen, so the protected linkage stays out of
/// the durable event log too. The caller writes it to the outbox inside the cast transaction.
#[must_use]
pub(crate) fn vote_cast_envelope(
    clock: &dyn Clock,
    org: OrgId,
    vote: VoteId,
    proposal: ProposalId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::VoteCast { vote, proposal },
    }
}

/// Build the `votes.tally.updated` envelope announcing the new privacy-safe aggregate. Downstream
/// consumers (e.g. `consensus`/`consequence`) react to the aggregate crossing a threshold; the
/// event carries the count only, never who voted.
#[must_use]
pub(crate) fn tally_updated_envelope(
    clock: &dyn Clock,
    org: OrgId,
    proposal: ProposalId,
    support_count: u64,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::VoteTallyUpdated {
            proposal,
            support_count,
        },
    }
}

/// Consume an inbound cross-crate event. Per its CRATE.md contract, `votes` subscribes to nothing,
/// so every delivery is ignored. The handler is total and side-effect-free, hence trivially
/// idempotent under at-least-once redelivery (ADR-0006). Returns whether the event was handled.
pub(crate) fn on_event(_envelope: &EventEnvelope) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::{DateTime, Utc};

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn vote_cast_envelope_carries_no_citizen_and_stamps_injected_time() {
        let clock = FixedClock(at());
        let org = OrgId::new();
        let vote = VoteId::new();
        let proposal = ProposalId::new();
        let env = vote_cast_envelope(&clock, org, vote, proposal);

        assert_eq!(env.at, at());
        assert_eq!(env.org, org);
        match env.event {
            Event::VoteCast {
                vote: v,
                proposal: p,
            } => {
                assert_eq!(v, vote);
                assert_eq!(p, proposal);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        // The serialized payload must not mention citizens — the linkage never enters the log.
        let json = serde_json::to_string(&env.event).unwrap();
        assert!(!json.contains("citizen"));
        assert!(json.contains("votes.cast"));
    }

    #[test]
    fn tally_updated_envelope_carries_count_only() {
        let clock = FixedClock(at());
        let proposal = ProposalId::new();
        let env = tally_updated_envelope(&clock, OrgId::new(), proposal, 7);
        match env.event {
            Event::VoteTallyUpdated {
                proposal: p,
                support_count,
            } => {
                assert_eq!(p, proposal);
                assert_eq!(support_count, 7);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let json = serde_json::to_string(&env.event).unwrap();
        assert!(!json.contains("citizen"));
    }

    #[test]
    fn consume_ignores_everything_idempotently() {
        let env = tally_updated_envelope(&FixedClock(at()), OrgId::new(), ProposalId::new(), 1);
        assert!(!on_event(&env).unwrap());
        // Redelivery is a no-op (same result), satisfying at-least-once idempotency.
        assert!(!on_event(&env).unwrap());
    }
}
