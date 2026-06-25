//! Test doubles shared across the workspace so crate-owner agents never block on a real
//! implementation (PLAN.md section 6.3). Available in normal builds (not behind a feature) so
//! downstream `dev-dependencies` can use them without extra wiring.

#![allow(clippy::expect_used)] // test-support code: lock poisoning is a test bug, panic is fine

use std::sync::Mutex;

use crate::error::Result;
use crate::events::EventEnvelope;
use crate::traits::EventBus;

/// An [`EventBus`] that records published envelopes in memory for assertions.
#[derive(Debug, Default)]
pub struct RecordingEventBus {
    published: Mutex<Vec<EventEnvelope>>,
}

impl RecordingEventBus {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of everything published so far.
    #[must_use]
    pub fn published(&self) -> Vec<EventEnvelope> {
        self.published
            .lock()
            .expect("recorder lock not poisoned")
            .clone()
    }

    /// Number of events published.
    #[must_use]
    pub fn count(&self) -> usize {
        self.published
            .lock()
            .expect("recorder lock not poisoned")
            .len()
    }
}

#[async_trait::async_trait]
impl EventBus for RecordingEventBus {
    async fn publish(&self, envelope: EventEnvelope) -> Result<()> {
        self.published
            .lock()
            .expect("recorder lock not poisoned")
            .push(envelope);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::events::Event;
    use crate::ids::{EventId, MandateId, OrgId, ProposalId};

    #[tokio::test]
    async fn records_published_events() {
        let bus = RecordingEventBus::new();
        bus.publish(EventEnvelope {
            id: EventId::new(),
            org: OrgId::new(),
            at: Utc::now(),
            event: Event::ProposalCreated {
                proposal: ProposalId::new(),
                mandate: MandateId::new(),
            },
        })
        .await
        .unwrap();
        assert_eq!(bus.count(), 1);
        assert!(matches!(
            bus.published()[0].event,
            Event::ProposalCreated { .. }
        ));
    }
}
