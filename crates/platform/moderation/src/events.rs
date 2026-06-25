//! The event seam: emit `moderation.flagged` / `moderation.cleared` after a decision is
//! persisted, and consume `proposals.created` / `comments.created` to drive evaluation.
//!
//! Cross-crate effects flow ONLY through the injected [`EventBus`] using the frozen
//! [`dsoc_core::events::Event`] catalog — never a direct call into a peer crate
//! (ARCHITECTURE.md section 2, ADR-0004).

use dsoc_core::clock::Clock;
use dsoc_core::error::Result;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, OrgId, ProposalId};
use dsoc_core::traits::EventBus;

use crate::domain::{Decision, Outcome};
use crate::service::ModerationService;

/// Publish the moderation verdict for a proposal-keyed subject. Called after the decision
/// row is durably written, so the emitted event always has a matching audit record.
///
/// # Errors
/// Propagates any [`dsoc_core::Error`] returned by the bus.
pub async fn emit_decision(
    bus: &dyn EventBus,
    clock: &dyn Clock,
    org: OrgId,
    proposal: ProposalId,
    outcome: Outcome,
) -> Result<()> {
    let event = match outcome {
        Outcome::Flagged => Event::ModerationFlagged { proposal },
        Outcome::Cleared => Event::ModerationCleared { proposal },
    };
    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event,
    };
    bus.publish(envelope).await
}

/// Consume one inbound event. `proposals.created` and `comments.created` trigger an
/// evaluation (returning the persisted [`Decision`]); any other event is ignored
/// (`Ok(None)`).
///
/// The artifact's text is not carried in the frozen event payload, so the dispatcher
/// supplies it as `content`. Every consumed event yields exactly one persisted decision —
/// decisions are never silently dropped.
///
/// # Errors
/// Propagates any [`dsoc_core::Error`] from evaluation/persistence.
pub async fn handle_event(
    service: &ModerationService,
    envelope: &EventEnvelope,
    content: &str,
) -> Result<Option<Decision>> {
    use crate::service::ModerationTarget;

    match envelope.event {
        Event::ProposalCreated { proposal, .. } => service
            .evaluate(envelope.org, ModerationTarget::Proposal(proposal), content)
            .await
            .map(Some),
        Event::CommentCreated { comment, proposal } => service
            .evaluate(
                envelope.org,
                ModerationTarget::Comment { comment, proposal },
                content,
            )
            .await
            .map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};
    use dsoc_core::clock::Clock;
    use dsoc_core::ids::ProposalId;
    use dsoc_core::testing::RecordingEventBus;

    use super::*;

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn fixed() -> FixedClock {
        FixedClock(
            DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
    }

    #[tokio::test]
    async fn emit_flagged_publishes_moderation_flagged() {
        let bus = Arc::new(RecordingEventBus::new());
        let clock = fixed();
        let org = OrgId::new();
        let proposal = ProposalId::new();
        emit_decision(bus.as_ref(), &clock, org, proposal, Outcome::Flagged)
            .await
            .unwrap();
        let published = bus.published();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].org, org);
        assert_eq!(published[0].at, clock.now());
        assert!(matches!(
            published[0].event,
            Event::ModerationFlagged { proposal: p } if p == proposal
        ));
    }

    #[tokio::test]
    async fn emit_cleared_publishes_moderation_cleared() {
        let bus = Arc::new(RecordingEventBus::new());
        let clock = fixed();
        let proposal = ProposalId::new();
        emit_decision(
            bus.as_ref(),
            &clock,
            OrgId::new(),
            proposal,
            Outcome::Cleared,
        )
        .await
        .unwrap();
        assert!(matches!(
            bus.published()[0].event,
            Event::ModerationCleared { proposal: p } if p == proposal
        ));
    }
}
