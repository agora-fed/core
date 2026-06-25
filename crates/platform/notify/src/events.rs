//! Event construction for `dsoc-notify`.
//!
//! Delivery outcomes are signalled to the rest of the system through the **transactional
//! outbox** (ADR-0006): [`crate::service`] writes the envelope built here into `events_log`
//! via `dsoc_db::outbox::publish_tx` **inside the same transaction** as the outbox-row state
//! change, so the domain write and its event commit atomically (no post-commit dual-write
//! window where a delivered/failed signal could be lost). The `dsoc-events` dispatcher then
//! delivers from `events_log` (at-least-once; consumers must be idempotent).
//!
//! This module stays free of `sqlx` and the `EventBus` port: it only maps a pure
//! [`EmitSignal`] onto the canonical `dsoc_core` event the service then persists.

use chrono::{DateTime, Utc};
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, NotificationId, OrgId};

use crate::domain::EmitSignal;

/// Build the terminal cross-crate event envelope a dispatch transition emits.
///
/// `Dispatched` → `notify.dispatched`; `Failed` → `notify.delivery.failed`. The caller writes
/// the returned envelope to the outbox within its open transaction.
#[must_use]
pub fn signal_envelope(
    org: OrgId,
    notification: NotificationId,
    at: DateTime<Utc>,
    signal: EmitSignal,
) -> EventEnvelope {
    let event = match signal {
        EmitSignal::Dispatched => Event::NotifyDispatched { notification },
        EmitSignal::Failed => Event::NotifyDeliveryFailed { notification },
    };
    EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn dispatched_signal_maps_to_notify_dispatched() {
        let org = OrgId::new();
        let notification = NotificationId::new();
        let at = Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap();
        let envelope = signal_envelope(org, notification, at, EmitSignal::Dispatched);
        assert_eq!(envelope.org, org);
        assert_eq!(envelope.at, at);
        assert!(matches!(
            envelope.event,
            Event::NotifyDispatched { notification: n } if n == notification
        ));
    }

    #[test]
    fn failed_signal_maps_to_delivery_failed() {
        let org = OrgId::new();
        let notification = NotificationId::new();
        let at = Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap();
        let envelope = signal_envelope(org, notification, at, EmitSignal::Failed);
        assert!(matches!(
            envelope.event,
            Event::NotifyDeliveryFailed { notification: n } if n == notification
        ));
    }
}
