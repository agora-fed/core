//! The event seam: build the durable `scorecard.updated` envelope the projection emits through the
//! **transactional outbox** (ADR-0006), and consume the consequence/mandate events that drive the
//! projection.
//!
//! Cross-crate effects flow ONLY through the frozen [`dsoc_core::events::Event`] catalog — never a
//! direct call into a peer crate (ARCHITECTURE.md section 2, ADR-0004). The emit half is written to
//! `events_log` inside the same transaction as the ledger append, so a counter change and its
//! `scorecard.updated` signal commit atomically; the consume half is idempotent because the ledger
//! dedupes by `sla_id` (delivery is at-least-once).

use std::sync::Arc;

use dsoc_core::error::{Error, Result};
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, MandateId, OrgId, ScorecardId};
use dsoc_core::Clock;

use crate::domain::ScorecardView;
use crate::service::ScorecardService;

/// Build a durable [`EventEnvelope`], stamping it with a fresh id, the org, and the injected clock's
/// time (never ambient — TESTING.md). The caller writes it to the outbox
/// (`dsoc_db::outbox::publish_tx`) inside the same transaction as the ledger append, so the change
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

/// The `scorecard.updated` event for a scorecard/mandate pair (the only event this crate emits).
#[must_use]
pub fn scorecard_updated(scorecard: ScorecardId, mandate: MandateId) -> Event {
    Event::ScorecardUpdated { scorecard, mandate }
}

/// Consume one inbound envelope, driving the projection. Delivery is at-least-once, so replays are
/// harmless: the ledger dedupes by `sla_id` and onboarding is an idempotent upsert.
///
/// - `mandates.official.onboarded` -> create a zeroed scorecard (emits `scorecard.updated`).
/// - `consequence.sla.expired` -> append an ignored entry and recount (emits `scorecard.updated`).
/// - `consequence.official.responded` -> append an answered entry with its latency and recount
///   (emits `scorecard.updated`). The latency is delivered alongside the slim event by the
///   dispatcher (the frozen catalog event carries ids only), mirroring how `consensus`/`moderation`
///   receive the proposal body; it is REQUIRED for a responded event.
/// - any other event -> ignored (`Ok(None)`), no write, no emission.
///
/// # Errors
/// [`Error::Validation`] when a responded event arrives without a latency; otherwise any
/// [`dsoc_core::Error`] from the projection write.
pub async fn handle_event(
    service: &ScorecardService,
    envelope: &EventEnvelope,
    response_hours: Option<f64>,
) -> Result<Option<ScorecardView>> {
    match envelope.event {
        Event::MandateOfficialOnboarded { mandate } => {
            service.on_onboarded(envelope.org, mandate).await.map(Some)
        }
        Event::ConsequenceSlaExpired { sla, mandate } => service
            .on_sla_expired(envelope.org, mandate, sla, envelope.at)
            .await
            .map(Some),
        Event::ConsequenceOfficialResponded { sla, mandate } => {
            let hours = response_hours.ok_or_else(|| {
                Error::Validation(
                    "consequence.official.responded requires a response latency".to_owned(),
                )
            })?;
            service
                .on_responded(envelope.org, mandate, sla, hours, envelope.at)
                .await
                .map(Some)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use dsoc_core::ids::{ProposalId, SlaId, VoteId};

    use super::*;

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn fixed() -> Arc<dyn Clock> {
        Arc::new(FixedClock(
            DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ))
    }

    #[test]
    fn envelope_stamps_org_and_clock_time() {
        let clock = fixed();
        let org = OrgId::new();
        let scorecard = ScorecardId::new();
        let mandate = MandateId::new();
        let env = envelope(&clock, org, scorecard_updated(scorecard, mandate));
        assert_eq!(env.org, org);
        assert_eq!(env.at, clock.now());
        assert!(matches!(
            env.event,
            Event::ScorecardUpdated { scorecard: s, mandate: m } if s == scorecard && m == mandate
        ));
        assert_eq!(env.event.topic().as_str(), "scorecard");
    }

    #[test]
    fn scorecard_updated_builds_the_catalog_event() {
        let scorecard = ScorecardId::new();
        let mandate = MandateId::new();
        let event = scorecard_updated(scorecard, mandate);
        assert!(matches!(
            event,
            Event::ScorecardUpdated { scorecard: s, mandate: m } if s == scorecard && m == mandate
        ));
    }

    // The unrelated-event filter and the responded-without-latency guard are documented here; the
    // full dispatch behaviour (which requires a database-backed service) is covered by the
    // integration tests. We assert the pure pattern-matching shape stays in the frozen catalog.
    #[test]
    fn frozen_catalog_variants_used_by_handler_exist() {
        let _expired = Event::ConsequenceSlaExpired {
            sla: SlaId::new(),
            mandate: MandateId::new(),
        };
        let _responded = Event::ConsequenceOfficialResponded {
            sla: SlaId::new(),
            mandate: MandateId::new(),
        };
        let _onboarded = Event::MandateOfficialOnboarded {
            mandate: MandateId::new(),
        };
        // A representative ignored event the handler must skip.
        let ignored = Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        };
        assert_eq!(ignored.topic().as_str(), "votes");
    }
}
