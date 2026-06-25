//! Event seam: build the durable envelope this crate emits through the **transactional
//! outbox** (ADR-0006), and consume `moderation.flagged` to drive the status fan-out.
//!
//! Cross-crate effects flow ONLY through the frozen [`dsoc_core::events::Event`] catalog —
//! never a direct call into a peer crate (ARCHITECTURE.md section 2, ADR-0004). The
//! emit-side builder is unit-tested here; the consume side is exercised end-to-end by the
//! integration tests that read `comment.status` after handling an envelope.

use std::sync::Arc;

use dsoc_core::error::Result;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CommentId, EventId, OrgId, ProposalId};
use dsoc_core::Clock;

use crate::service::CommentService;

/// Build the `comments.created` envelope, stamping it with a fresh id, the org, and the
/// injected clock's time (never ambient — TESTING.md). The caller writes it to the outbox
/// (`dsoc_db::outbox::publish_tx`) inside the same transaction as the comment insert, so
/// the change and the event commit atomically (ADR-0006).
#[must_use]
pub fn comment_created_envelope(
    clock: &Arc<dyn Clock>,
    org: OrgId,
    comment: CommentId,
    proposal: ProposalId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::CommentCreated { comment, proposal },
    }
}

/// Extract the proposal a `moderation.flagged` envelope targets, ignoring every other
/// event type. This is the consume-side filter (the catalog event carries ids only).
#[must_use]
pub fn moderation_flagged(envelope: &EventEnvelope) -> Option<ProposalId> {
    match envelope.event {
        Event::ModerationFlagged { proposal } => Some(proposal),
        _ => None,
    }
}

/// Consume one inbound event. A `moderation.flagged` envelope flags every still-visible
/// comment of the targeted proposal (returning how many were transitioned); any other
/// event is ignored (`Ok(None)`).
///
/// Delivery is at-least-once, so this is idempotent: the underlying UPDATE is guarded by
/// the `visible` prior state, so a redundant delivery transitions zero rows and returns
/// `Ok(Some(0))` rather than double-flagging or erroring.
///
/// # Errors
/// Propagates any [`dsoc_core::Error`] from the status fan-out.
pub async fn handle_event(
    service: &CommentService,
    envelope: &EventEnvelope,
) -> Result<Option<u64>> {
    match moderation_flagged(envelope) {
        Some(proposal) => service
            .flag_proposal_comments(envelope.org, proposal)
            .await
            .map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use dsoc_core::ids::{MandateId, VoteId};

    use super::*;

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn fixed_clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock(
            DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ))
    }

    fn envelope(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn builds_comment_created_with_injected_time() {
        let clock = fixed_clock();
        let org = OrgId::new();
        let comment = CommentId::new();
        let proposal = ProposalId::new();
        let env = comment_created_envelope(&clock, org, comment, proposal);
        assert_eq!(env.org, org);
        assert_eq!(env.at, clock.now());
        assert!(matches!(
            env.event,
            Event::CommentCreated { comment: c, proposal: p } if c == comment && p == proposal
        ));
        assert_eq!(env.event.topic().as_str(), "comments");
    }

    #[test]
    fn extracts_moderation_flagged_proposal() {
        let proposal = ProposalId::new();
        let env = envelope(Event::ModerationFlagged { proposal });
        assert_eq!(moderation_flagged(&env), Some(proposal));
    }

    #[test]
    fn ignores_unrelated_events() {
        assert_eq!(
            moderation_flagged(&envelope(Event::ModerationCleared {
                proposal: ProposalId::new(),
            })),
            None
        );
        assert_eq!(
            moderation_flagged(&envelope(Event::VoteCast {
                vote: VoteId::new(),
                proposal: ProposalId::new(),
            })),
            None
        );
        assert_eq!(
            moderation_flagged(&envelope(Event::ProposalCreated {
                proposal: ProposalId::new(),
                mandate: MandateId::new(),
            })),
            None
        );
    }
}
