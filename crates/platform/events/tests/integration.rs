//! Integration tests for `dsoc-events` against a real PostgreSQL (`$DATABASE_URL`).
//!
//! Each test isolates its data with a fresh `org` and unique ids/topics/subscribers, so the
//! suite is safe under parallelism and re-runs. Time is injected via a `FixedClock` (TESTING.md:
//! never sleep). The durable bus is driven through its public API and the shared
//! [`dsoc_core::testing::RecordingEventBus`].

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope, EventTopic};
use dsoc_core::ids::{
    CitizenId, ClusterId, CommentId, EventId, MandateId, OrgId, ProposalId, ScorecardId, VoteId,
};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::traits::EventBus;
use dsoc_db::Db;
use dsoc_events::{dispatch_batch, EventHandler, EventQueue, PgEventBus, SubscriberName};

/// A deterministic clock for tests (TESTING.md: SLA/time logic is injected, never ambient).
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
    ))
}

/// An [`EventHandler`] that forwards each delivery into a [`RecordingEventBus`] for assertions.
#[derive(Debug)]
struct ForwardingHandler {
    bus: Arc<RecordingEventBus>,
}

#[async_trait::async_trait]
impl EventHandler for ForwardingHandler {
    async fn handle(&self, envelope: &EventEnvelope) -> dsoc_core::Result<()> {
        self.bus.publish(envelope.clone()).await
    }
}

async fn pool() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 2).await.expect("connect to test db")
}

/// Insert a fresh org and return its id (satisfies the `events_log.org_id` FK).
async fn fresh_org(db: &Db, at: DateTime<Utc>) -> OrgId {
    let org = OrgId::new();
    sqlx::query!(
        r#"INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)"#,
        org.as_uuid(),
        format!("evt-test-{}", org.as_uuid()),
        "events integration test",
        at,
    )
    .execute(db)
    .await
    .expect("insert org");
    org
}

fn envelope(org: OrgId, at: DateTime<Utc>, event: Event) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event,
    }
}

#[tokio::test]
async fn publish_inserts_row_and_roundtrips_event_json() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());

    let event = Event::ProposalCreated {
        proposal: ProposalId::new(),
        mandate: MandateId::new(),
    };
    let env = envelope(org, now, event.clone());
    bus.publish(env.clone()).await.expect("publish");

    let row = sqlx::query!(
        r#"SELECT topic, event_type, payload::text AS "payload!", occurred_at
           FROM events_log WHERE id = $1"#,
        env.id.as_uuid(),
    )
    .fetch_one(&db)
    .await
    .expect("row exists");

    assert_eq!(row.topic, "proposals");
    assert_eq!(row.event_type, "proposals.created");
    assert_eq!(row.occurred_at, now);
    let decoded = dsoc_events::decode_event(&row.payload).expect("decode");
    assert_eq!(decoded, event);
}

#[tokio::test]
async fn unprocessed_poll_returns_published_event() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());
    let queue = EventQueue::new(db.clone(), clock.clone());

    let env = envelope(
        org,
        now,
        Event::CommentCreated {
            comment: CommentId::new(),
            proposal: ProposalId::new(),
        },
    );
    bus.publish(env.clone()).await.expect("publish");

    let polled = queue
        .poll(EventTopic::Comments, None, 500)
        .await
        .expect("poll");
    assert!(
        polled.iter().any(|e| e.id == env.id),
        "freshly published comment must appear in the unprocessed poll"
    );
}

#[tokio::test]
async fn mark_processed_sets_processed_at_and_is_idempotent() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());
    let queue = EventQueue::new(db.clone(), clock.clone());

    let env = envelope(
        org,
        now,
        Event::AuthVerificationUpgraded {
            citizen: CitizenId::new(),
        },
    );
    bus.publish(env.clone()).await.expect("publish");

    queue.mark_processed(env.id).await.expect("first mark");
    // Idempotent: a second mark must still succeed without error.
    queue.mark_processed(env.id).await.expect("second mark");

    let row = sqlx::query!(
        r#"SELECT processed_at FROM events_log WHERE id = $1"#,
        env.id.as_uuid(),
    )
    .fetch_one(&db)
    .await
    .expect("row exists");
    assert!(row.processed_at.is_some(), "processed_at must be set");
}

#[tokio::test]
async fn mark_processed_unknown_event_is_not_found() {
    let db = pool().await;
    let queue = EventQueue::new(db, fixed_clock());
    let err = queue
        .mark_processed(EventId::new())
        .await
        .expect_err("unknown event must error");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn topic_routing_matches_event_topic() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());
    let queue = EventQueue::new(db.clone(), clock.clone());

    let env = envelope(
        org,
        now,
        Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        },
    );
    bus.publish(env.clone()).await.expect("publish");

    let votes = queue
        .poll(EventTopic::Votes, None, 500)
        .await
        .expect("poll");
    assert!(
        votes.iter().any(|e| e.id == env.id),
        "vote must route to the votes topic"
    );

    let moderation = queue
        .poll(EventTopic::Moderation, None, 500)
        .await
        .expect("poll");
    assert!(
        !moderation.iter().any(|e| e.id == env.id),
        "vote must NOT appear under an unrelated topic"
    );
}

#[tokio::test]
async fn queue_depth_counts_unprocessed_for_topic() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());
    let queue = EventQueue::new(db.clone(), clock.clone());

    bus.publish(envelope(
        org,
        now,
        Event::ConsensusClusterFormed {
            cluster: ClusterId::new(),
            size: 3,
        },
    ))
    .await
    .expect("publish");

    let depths = queue.queue_depth().await.expect("queue depth");
    let consensus = depths
        .iter()
        .find(|d| d.topic == "consensus")
        .expect("consensus topic present");
    assert!(
        consensus.pending >= 1,
        "consensus backlog must include the freshly published event"
    );
}

#[tokio::test]
async fn dispatch_batch_drives_handler_and_advances_cursor() {
    let db = pool().await;
    let clock = fixed_clock();
    let now = clock.now();
    let org = fresh_org(&db, now).await;
    let bus = PgEventBus::new(db.clone());
    let queue = EventQueue::new(db.clone(), clock.clone());

    // Unique subscriber so the durable cursor starts fresh (no leakage across runs).
    let subscriber =
        SubscriberName::parse(&format!("scorecard-worker-{}", OrgId::new().as_uuid())).unwrap();

    let first = envelope(
        org,
        now,
        Event::ScorecardUpdated {
            scorecard: ScorecardId::new(),
            mandate: MandateId::new(),
        },
    );
    let second = envelope(
        org,
        now,
        Event::ScorecardUpdated {
            scorecard: ScorecardId::new(),
            mandate: MandateId::new(),
        },
    );
    bus.publish(first.clone()).await.expect("publish first");
    bus.publish(second.clone()).await.expect("publish second");

    let recorder = Arc::new(RecordingEventBus::new());
    let handler = ForwardingHandler {
        bus: recorder.clone(),
    };

    let handled = dispatch_batch(&queue, &subscriber, EventTopic::Scorecard, 500, &handler)
        .await
        .expect("dispatch");
    assert!(handled >= 2, "both published events must be handled");

    let recorded: Vec<EventId> = recorder.published().iter().map(|e| e.id).collect();
    assert!(recorded.contains(&first.id));
    assert!(recorded.contains(&second.id));

    // Both deliveries are now durably processed.
    for id in [first.id, second.id] {
        let row = sqlx::query!(
            r#"SELECT processed_at FROM events_log WHERE id = $1"#,
            id.as_uuid(),
        )
        .fetch_one(&db)
        .await
        .expect("row exists");
        assert!(row.processed_at.is_some());
    }

    // The cursor advanced; a second dispatch finds nothing new for this subscriber.
    let again = dispatch_batch(&queue, &subscriber, EventTopic::Scorecard, 500, &handler)
        .await
        .expect("dispatch again");
    assert_eq!(again, 0, "cursor must advance past handled deliveries");
}
