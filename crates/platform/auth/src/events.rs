//! Cross-crate event emission and consumption. The crate only ever touches the **frozen**
//! `dsoc_core::events` catalog and publishes through the injected `Arc<dyn EventBus>` — it
//! never depends on `dsoc-events` (ADR-0004 wiring conventions).

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, EventId, OrgId};
use dsoc_core::{Clock, Result};

use crate::domain;

/// Build the `auth.verification.upgraded` envelope for a citizen whose assurance level rose. The
/// envelope timestamp comes from the injected [`Clock`], never the ambient wall clock (TESTING.md).
/// The caller emits it through the transactional outbox (ADR-0006) inside the same transaction as
/// the audit write, so the level change and its event commit atomically.
pub(crate) fn verification_upgraded_envelope(
    clock: &dyn Clock,
    org: OrgId,
    citizen: CitizenId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::AuthVerificationUpgraded { citizen },
    }
}

/// Consume an inbound event. The crate reacts only to `mandates.official.invited`: an invited
/// official is a directory-verifiable identity, recorded here so the audit trail shows the
/// signal was received. Returns whether the event was handled by this crate (consumers must be
/// idempotent — re-delivery is a no-op).
pub(crate) fn on_event(envelope: &EventEnvelope) -> Result<bool> {
    if !domain::consumes(&envelope.event) {
        return Ok(false);
    }
    if let Event::MandateOfficialInvited { mandate } = &envelope.event {
        tracing::info!(
            org = %envelope.org,
            mandate = %mandate,
            "auth observed mandates.official.invited; directory verification is pending the official's first login",
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::Utc;
    use dsoc_core::ids::{MandateId, ProposalId};

    fn envelope(event: Event) -> EventEnvelope {
        EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn consumes_official_invited() {
        let env = envelope(Event::MandateOfficialInvited {
            mandate: MandateId::new(),
        });
        assert!(on_event(&env).unwrap());
    }

    #[test]
    fn ignores_unrelated_event() {
        let env = envelope(Event::ModerationFlagged {
            proposal: ProposalId::new(),
        });
        assert!(!on_event(&env).unwrap());
    }
}
