//! Event wiring: build the durable envelope consensus emits through the **transactional outbox**
//! (ADR-0006), and extract the proposal carried by a consumed `proposals.created` envelope. Only
//! existing `dsoc_core::events::Event` variants are used (the catalog is frozen — ADR-0004). The
//! extraction half is pure and unit-tested; outbox emission is exercised by the integration tests
//! that read `events_log`.

use std::sync::Arc;

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, MandateId, OrgId, ProposalId};
use dsoc_core::Clock;

/// Build a durable [`EventEnvelope`], stamping it with a fresh id, the org, and the injected clock's
/// time (never ambient — TESTING.md). The caller writes it to the outbox
/// (`dsoc_db::outbox::publish_tx`) inside the same transaction as the domain write, so the change
/// and the event commit atomically (ADR-0006).
#[must_use]
pub fn envelope(clock: &Arc<dyn Clock>, org: OrgId, event: Event) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event,
    }
}

/// Extract `(proposal, mandate)` from a `proposals.created` envelope, ignoring every other event
/// type. This is the consume-side filter; the proposal body to embed is delivered alongside the
/// slim event by the caller (the catalog event carries ids only).
#[must_use]
pub fn proposal_created(envelope: &EventEnvelope) -> Option<(ProposalId, MandateId)> {
    match envelope.event {
        Event::ProposalCreated { proposal, mandate } => Some((proposal, mandate)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use dsoc_core::ids::VoteId;

    fn envelope(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn extracts_proposal_created() {
        let proposal = ProposalId::new();
        let mandate = MandateId::new();
        let env = envelope(Event::ProposalCreated { proposal, mandate });
        assert_eq!(proposal_created(&env), Some((proposal, mandate)));
    }

    #[test]
    fn ignores_other_events() {
        let env = envelope(Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        });
        assert_eq!(proposal_created(&env), None);
    }
}
