//! Cross-crate event emission and consumption. The crate only ever touches the **frozen**
//! `dsoc_core::events` catalog; mutations emit through the **transactional outbox** (ADR-0006)
//! inside the same transaction as the write, never a post-commit `EventBus::publish`. It never
//! depends on `dsoc-events` (ADR-0004 wiring conventions).

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, MandateId, OrgId};
use dsoc_core::{Clock, Result};

use crate::domain;

/// Build the `mandates.official.invited` envelope. The timestamp comes from the injected
/// [`Clock`], never the ambient wall clock (docs/TESTING.md). The caller writes it via the
/// transactional outbox in the same transaction as the invitation insert.
pub(crate) fn official_invited_envelope(
    clock: &dyn Clock,
    org: OrgId,
    mandate: MandateId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::MandateOfficialInvited { mandate },
    }
}

/// Build the `mandates.official.onboarded` envelope (emitted when an official accepts a valid
/// invite and the mandate transitions to onboarded), committed atomically with the transition.
pub(crate) fn official_onboarded_envelope(
    clock: &dyn Clock,
    org: OrgId,
    mandate: MandateId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::MandateOfficialOnboarded { mandate },
    }
}

/// Build the `mandates.identity.verified` envelope (emitted when an identity binding is recorded
/// at a higher assurance level), committed atomically with the binding insert.
pub(crate) fn identity_verified_envelope(
    clock: &dyn Clock,
    org: OrgId,
    mandate: MandateId,
) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: clock.now(),
        event: Event::MandateIdentityVerified { mandate },
    }
}

/// Consume an inbound event. The crate reacts only to `auth.verification.upgraded`: a citizen
/// reaching a higher assurance level is observed here (it may later corroborate a mandate's
/// identity binding). Returns whether this crate handled the event. Consumers must be IDEMPOTENT —
/// delivery is at-least-once, so re-delivery is a harmless no-op (the handler only logs).
pub(crate) fn on_event(envelope: &EventEnvelope) -> Result<bool> {
    if !domain::consumes(&envelope.event) {
        return Ok(false);
    }
    if let Event::AuthVerificationUpgraded { citizen } = &envelope.event {
        tracing::info!(
            org = %envelope.org,
            citizen = %citizen,
            "mandates observed auth.verification.upgraded; identity corroboration is recorded on the next binding",
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::{DateTime, Utc};
    use dsoc_core::ids::{CitizenId, ProposalId};

    #[derive(Debug)]
    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn clock() -> FixedClock {
        FixedClock(
            DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
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
    fn invited_envelope_carries_clock_time_and_event() {
        let mandate = MandateId::new();
        let env = official_invited_envelope(&clock(), OrgId::new(), mandate);
        assert_eq!(env.at, clock().0);
        assert!(matches!(
            env.event,
            Event::MandateOfficialInvited { mandate: m } if m == mandate
        ));
    }

    #[test]
    fn onboarded_envelope_carries_event() {
        let mandate = MandateId::new();
        let env = official_onboarded_envelope(&clock(), OrgId::new(), mandate);
        assert!(matches!(
            env.event,
            Event::MandateOfficialOnboarded { mandate: m } if m == mandate
        ));
    }

    #[test]
    fn identity_verified_envelope_carries_event() {
        let mandate = MandateId::new();
        let env = identity_verified_envelope(&clock(), OrgId::new(), mandate);
        assert!(matches!(
            env.event,
            Event::MandateIdentityVerified { mandate: m } if m == mandate
        ));
    }

    #[test]
    fn consumes_auth_verification_upgraded() {
        let env = envelope(Event::AuthVerificationUpgraded {
            citizen: CitizenId::new(),
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
