//! Integration test for the transactional outbox against real PostgreSQL.
use chrono::Utc;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{EventId, MandateId, OrgId, ProposalId};

#[tokio::test]
async fn publish_tx_writes_a_roundtrippable_events_log_row() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL set for db integration tests");
    let pool = dsoc_db::connect(&url, 2).await.expect("connect");

    // an org to satisfy the FK
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1,$2,$3,$4)")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid()))
        .bind("Test Org")
        .bind(Utc::now())
        .execute(&pool)
        .await
        .expect("seed org");

    let env = EventEnvelope {
        id: EventId::new(),
        org,
        at: Utc::now(),
        event: Event::ProposalCreated {
            proposal: ProposalId::new(),
            mandate: MandateId::new(),
        },
    };

    let mut tx = pool.begin().await.expect("begin");
    dsoc_db::outbox::publish_tx(&mut *tx, &env)
        .await
        .expect("publish in tx");
    tx.commit().await.expect("commit");

    // the row exists, routes under the right topic, and the payload round-trips back to Event
    let (topic, event_type, payload): (String, String, String) =
        sqlx::query_as("SELECT topic, event_type, payload::text FROM events_log WHERE id = $1")
            .bind(env.id.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("fetch outbox row");

    assert_eq!(topic, "proposals");
    assert_eq!(event_type, "proposals.created");
    let back: Event = serde_json::from_str(&payload).expect("payload round-trips to Event");
    assert_eq!(back, env.event);
}
