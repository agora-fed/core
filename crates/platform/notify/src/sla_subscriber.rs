//! SLA-lifecycle subscriber: turns the three consequence-loop events into notifications for the
//! citizen who **operates** the targeted mandate.
//!
//! The bus (`dsoc-events`) drains at-least-once; the composition root (the gateway worker) wires
//! this handler onto the topics that carry these events. The handler:
//!
//! 1. Reads `mandate_identity_binding` for the mandate on the envelope to discover the operator's
//!    `citizen_id`. No binding → info log and skip (nobody to notify yet — the mandate hasn't
//!    onboarded a real operator, which is expected during the F1 rollout).
//! 2. Delegates to [`crate::service::NotifyService::enqueue_for_event`], which consults
//!    [`crate::domain::plan_for_event`] to pick the channel/body. This crate then dispatches on
//!    its normal cadence (in-app / email — whatever the deployed transports cover). No new
//!    channel is introduced here.
//!
//! Handled events (per ADR-0004 catalog):
//! - `proposals.threshold.crossed` — a directed proposal crossed its threshold. The consequence
//!   loop is about to start an SLA clock; forewarn the operator so the response window doesn't
//!   catch them cold.
//! - `consequence.sla.started` — the SLA clock started (the crate contract already lists this).
//! - `consequence.sla.expired` — silence recorded (the crate contract already lists this).
//!
//! Idempotent: enqueue writes an outbox row keyed on a fresh id, so replays add rows (each
//! delivery is bounded by `MAX_DELIVERY_ATTEMPTS`) — a design the notify contract already assumes.

use dsoc_core::error::Result;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, MandateId, NotificationId};
use dsoc_db::Db;
use uuid::Uuid;

use crate::service::NotifyService;

/// Extract the mandate id an SLA-lifecycle event references. Returns `None` for events this
/// subscriber does not act on (so the handler is total over the catalog).
#[must_use]
fn mandate_for(event: &Event) -> Option<MandateId> {
    match *event {
        Event::ProposalThresholdCrossed { mandate, .. }
        | Event::ConsequenceSlaStarted { mandate, .. }
        | Event::ConsequenceSlaExpired { mandate, .. } => Some(mandate),
        _ => None,
    }
}

/// Look up the citizen operating a mandate (the most-recently verified binding row that carries a
/// citizen id). Returns `None` when no operator is bound yet — a real state during rollout, never
/// an error. Runtime `sqlx::query_as` (not the macro) so the offline `.sqlx/` cache does not need
/// regenerating on a DB-less build host (mirrors `parlamentar_activity::load_mandate_source`).
async fn find_operator(db: &Db, mandate_id: Uuid) -> Result<Option<CitizenId>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT citizen_id FROM mandate_identity_binding \
         WHERE mandate_id = $1 AND citizen_id IS NOT NULL \
         ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(mandate_id)
    .fetch_optional(db)
    .await
    .map_err(|e| dsoc_core::error::Error::Storage(Box::new(e)))?;
    Ok(row.map(|(id,)| CitizenId::from_uuid(id)))
}

/// Consume one inbound envelope. Returns the enqueued notification id when a fan-out was written,
/// `None` when the event is ignored or the mandate has no operator to notify yet.
///
/// # Errors
/// Propagates any error from the DB lookup or the notify service.
pub async fn handle_event(
    service: &NotifyService,
    db: &Db,
    envelope: &EventEnvelope,
) -> Result<Option<NotificationId>> {
    let Some(mandate) = mandate_for(&envelope.event) else {
        return Ok(None);
    };
    let Some(operator) = find_operator(db, mandate.as_uuid()).await? else {
        tracing::info!(
            mandate = %mandate.as_uuid(),
            event = ?envelope.event,
            "sla notify: no bound operator; skipping"
        );
        return Ok(None);
    };
    service.enqueue_for_event(envelope, operator).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dsoc_core::events::EventEnvelope;
    use dsoc_core::ids::{ClusterId, EventId, MandateId, OrgId, ProposalId, SlaId};

    fn env(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap(),
            event,
        }
    }

    #[test]
    fn mandate_for_extracts_threshold_crossed() {
        let m = MandateId::new();
        let e = env(Event::ProposalThresholdCrossed {
            proposal: ProposalId::new(),
            cluster: ClusterId::new(),
            mandate: m,
        });
        assert_eq!(mandate_for(&e.event), Some(m));
    }

    #[test]
    fn mandate_for_extracts_sla_started() {
        let m = MandateId::new();
        let e = env(Event::ConsequenceSlaStarted {
            sla: SlaId::new(),
            mandate: m,
            cluster: ClusterId::new(),
            due: Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap(),
        });
        assert_eq!(mandate_for(&e.event), Some(m));
    }

    #[test]
    fn mandate_for_extracts_sla_expired() {
        let m = MandateId::new();
        let e = env(Event::ConsequenceSlaExpired {
            sla: SlaId::new(),
            mandate: m,
        });
        assert_eq!(mandate_for(&e.event), Some(m));
    }

    #[test]
    fn mandate_for_ignores_unrelated_events() {
        let e = env(Event::VoteCast {
            vote: dsoc_core::ids::VoteId::new(),
            proposal: ProposalId::new(),
        });
        assert_eq!(mandate_for(&e.event), None);
    }
}
