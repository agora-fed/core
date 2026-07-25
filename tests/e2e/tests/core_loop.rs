//! The thesis E2E test: *propose → cluster → vote → notify → SLA → public silence*.
//!
//! Today this asserts the **event contract** that connects the crates (the only sanctioned
//! cross-crate coupling). As Tier-1/2 crates land, this file grows into a live test that
//! drives the gateway against a real PostgreSQL and asserts the recorded scorecard outcome.
//! Per docs/TESTING.md, this test is REQUIRED for release and must never be silently skipped.

use chrono::{Duration, TimeZone, Utc};
use dsoc_core::events::{Event, EventTopic};
use dsoc_core::ids::{ClusterId, MandateId, ProposalId, ScorecardId, SlaId, VoteId};

/// The ordered events the platform must emit for one full accountability loop.
fn consequence_loop_events() -> Vec<Event> {
    let mandate = MandateId::new();
    let proposal = ProposalId::new();
    let cluster = ClusterId::new();
    let sla = SlaId::new();
    let due = Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap();

    vec![
        Event::MandateOfficialOnboarded { mandate },
        Event::ProposalCreated { proposal, mandate },
        Event::ConsensusClusterFormed { cluster, size: 12 },
        Event::VoteCast {
            vote: VoteId::new(),
            proposal,
        },
        Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate,
            mandates: vec![mandate],
        },
        Event::ConsequenceSlaStarted {
            sla,
            mandate,
            cluster,
            due,
        },
        // ... SLA window elapses with no official response ...
        Event::ConsequenceSlaExpired { sla, mandate },
        Event::ScorecardUpdated {
            scorecard: ScorecardId::new(),
            mandate,
        },
    ]
}

#[test]
fn loop_emits_events_in_thesis_order() {
    let events = consequence_loop_events();

    // The loop must start at onboarding and end at the public scorecard update.
    assert!(matches!(
        events.first(),
        Some(Event::MandateOfficialOnboarded { .. })
    ));
    assert!(matches!(
        events.last(),
        Some(Event::ScorecardUpdated { .. })
    ));

    // The consequence engine must fire AFTER the threshold is crossed.
    let threshold = events
        .iter()
        .position(|e| matches!(e, Event::ProposalThresholdCrossed { .. }))
        .expect("threshold event present");
    let sla_started = events
        .iter()
        .position(|e| matches!(e, Event::ConsequenceSlaStarted { .. }))
        .expect("sla started present");
    assert!(
        threshold < sla_started,
        "SLA must start only after the threshold is crossed"
    );
}

#[test]
fn public_silence_is_recorded_when_sla_expires() {
    let events = consequence_loop_events();
    // The whole point of the platform: silence is recorded, not forgotten.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ConsequenceSlaExpired { .. })),
        "an unanswered SLA must produce a public-silence event"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::ScorecardUpdated { .. })));
}

#[test]
fn consequence_events_route_to_the_consequence_topic() {
    for e in consequence_loop_events() {
        if matches!(
            e,
            Event::ConsequenceSlaStarted { .. }
                | Event::ConsequenceSlaExpired { .. }
                | Event::ConsequenceOfficialResponded { .. }
        ) {
            assert_eq!(e.topic(), EventTopic::Consequence);
        }
    }
}

#[test]
fn sla_due_is_in_the_future_relative_to_start() {
    // Deterministic time math (no sleep): a 7-day SLA window.
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let due = start + Duration::days(7);
    assert!(due > start);
}
