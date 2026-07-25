//! Integration tests for `consequence` against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! This is the accountability thesis under test. Each case isolates its data with a unique `org_id`
//! and a freshly-seeded `mandate`, uses a deterministic [`FixedClock`] (NEVER sleeps), and drives
//! expiry through `sweep_expired(now)` with an explicit injected time. Events are emitted through the
//! transactional outbox (ADR-0006), so the tests assert against the `events_log` rows the service
//! wrote inside the same transaction as the domain change — proving atomicity, not trusting a
//! post-commit publish.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use dsoc_api_contract::SlaStatus;
use dsoc_consequence::{ConsequenceService, SlaPolicy, ThresholdCrossed};
use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{ClusterId, EventId, MandateId, OrgId, ProposalId, SlaId};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// A deterministic clock: time is injected, never read ambiently (TESTING.md).
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// The clock anchor every test starts from (UTC).
fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

fn clock_at(at: DateTime<Utc>) -> Arc<dyn Clock> {
    Arc::new(FixedClock(at))
}

/// A 72-hour policy (the production default), used by most tests.
fn policy() -> SlaPolicy {
    SlaPolicy::new(Duration::hours(72))
}

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to PostgreSQL")
}

/// Seed an `org` row (FK target for `consequence_sla.org_id` and `events_log.org_id`).
async fn seed_org(db: &PgPool) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, now())")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid()))
        .bind("Município de Teste")
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Seed a `mandate` row (FK target for `consequence_sla.mandate_id` / `consequence_response`).
async fn seed_mandate(db: &PgPool, org: OrgId) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, is_candidate, created_at)
         VALUES ($1, $2, 'vereador', 'Vereadora Teste', 'gabinete@example.org', false, now())",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

fn service(db: PgPool, at: DateTime<Utc>) -> ConsequenceService {
    ConsequenceService::new(db, clock_at(at), policy())
}

fn threshold(proposal: ProposalId, cluster: ClusterId, mandate: MandateId) -> ThresholdCrossed {
    ThresholdCrossed {
        proposal,
        cluster,
        mandate,
    }
}

/// Read the event types the service wrote to the outbox for `org`, ordered by the (time-ordered)
/// event id so the sequence is deterministic.
async fn outbox_event_types(db: &PgPool, org: OrgId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events_log WHERE org_id = $1 ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox events")
}

/// Read the `due` timestamp carried by the single `consequence.sla.started` event for `org`.
async fn started_event_due(db: &PgPool, org: OrgId) -> DateTime<Utc> {
    let payload = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload FROM events_log \
         WHERE org_id = $1 AND event_type = 'consequence.sla.started' ORDER BY id LIMIT 1",
    )
    .bind(org.as_uuid())
    .fetch_one(db)
    .await
    .expect("read started event payload");
    let due = payload["data"]["due"]
        .as_str()
        .expect("started event carries a due timestamp");
    DateTime::parse_from_rfc3339(due)
        .expect("due is rfc3339")
        .with_timezone(&Utc)
}

#[tokio::test]
async fn threshold_starts_sla_with_due_from_clock_and_emits_started_carrying_due() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");

    assert!(started.newly_started, "a fresh threshold starts the clock");
    assert_eq!(started.sla.status, "pending");
    // due = clock.now() + window (72h).
    assert_eq!(started.sla.due_at, t0() + Duration::hours(72));

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consequence.sla.started".to_string()]
    );
    // The emitted Started event carries the due time computed from the injected clock.
    assert_eq!(
        started_event_due(&db, org).await,
        t0() + Duration::hours(72)
    );
}

#[tokio::test]
async fn expiry_past_due_records_ignored_outcome_and_emits_expired_without_sleep() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    // Drive time forward by passing an explicit `now` past the due time — no sleep.
    let expired = svc
        .sweep_expired(org, t0() + Duration::hours(73))
        .await
        .expect("sweep");
    assert_eq!(expired, 1, "exactly one SLA expired into silence");

    let row = svc.get_sla(sla_id).await.expect("reload sla");
    assert_eq!(row.status, "ignored", "public silence recorded");
    assert_eq!(
        svc.outcome(sla_id).await.expect("outcome"),
        Some("ignored".to_string()),
        "write-once outcome records permanent silence"
    );

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "consequence.sla.started".to_string(),
            "consequence.sla.expired".to_string(),
        ]
    );
}

#[tokio::test]
async fn response_before_due_answers_and_later_tick_does_not_expire() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    // Clock at t0: the response happens before the 72h due time.
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    let outcome = svc
        .respond(sla_id, "Recebido; vou analisar a demanda.", false)
        .await
        .expect("respond before due");
    assert_eq!(outcome.status, SlaStatus::Answered);

    // A later sweep past the original due time must NOT expire an already-answered SLA.
    let expired = svc
        .sweep_expired(org, t0() + Duration::hours(100))
        .await
        .expect("sweep after answer");
    assert_eq!(expired, 0, "an answered SLA is never expired");

    let row = svc.get_sla(sla_id).await.expect("reload");
    assert_eq!(row.status, "answered");
    assert_eq!(svc.outcome(sla_id).await.unwrap(), Some("answered".into()));

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "consequence.sla.started".to_string(),
            "consequence.official.responded".to_string(),
        ],
        "no expired event is ever emitted after an answer"
    );
}

#[tokio::test]
async fn response_with_commitment_records_acted() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    let outcome = svc
        .respond(sla_id, "Vou protocolar o projeto na próxima sessão.", true)
        .await
        .expect("respond committed");
    assert_eq!(
        outcome.status,
        SlaStatus::Acted,
        "a commitment records Acted"
    );

    assert_eq!(svc.outcome(sla_id).await.unwrap(), Some("acted".into()));
    assert_eq!(svc.get_sla(sla_id).await.unwrap().status, "acted");
}

#[tokio::test]
async fn response_after_expiry_conflicts_and_outcome_stays_ignored() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    // Silence is recorded first.
    svc.sweep_expired(org, t0() + Duration::hours(73))
        .await
        .expect("sweep");
    assert_eq!(svc.outcome(sla_id).await.unwrap(), Some("ignored".into()));

    // A late response must conflict — the public record of silence is permanent.
    let err = svc
        .respond(sla_id, "Desculpe a demora, segue resposta.", false)
        .await
        .expect_err("responding after silence must conflict");
    assert_eq!(err.code(), "conflict");

    // The outcome is unchanged: still ignored.
    assert_eq!(
        svc.outcome(sla_id).await.unwrap(),
        Some("ignored".into()),
        "recorded silence is permanent"
    );
    assert_eq!(svc.get_sla(sla_id).await.unwrap().status, "ignored");

    // No `responded` event was emitted by the rejected attempt.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "consequence.sla.started".to_string(),
            "consequence.sla.expired".to_string(),
        ]
    );
}

#[tokio::test]
async fn duplicate_threshold_event_does_not_start_two_slas() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let proposal = ProposalId::new();
    let cluster = ClusterId::new();

    let first = svc
        .start_sla(org, threshold(proposal, cluster, mandate))
        .await
        .expect("first threshold");
    assert!(first.newly_started);

    // The same (proposal, cluster) crosses again (at-least-once delivery): idempotent no-op.
    let second = svc
        .start_sla(org, threshold(proposal, cluster, mandate))
        .await
        .expect("duplicate threshold");
    assert!(
        !second.newly_started,
        "a duplicate must not start a new clock"
    );
    assert_eq!(
        second.sla.id, first.sla.id,
        "the duplicate returns the REAL stored row id, not a phantom one"
    );

    // Exactly one SLA exists for the org, and exactly one started event was emitted.
    let (rows, total) = svc.list_slas(org, None, 20).await.expect("list");
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consequence.sla.started".to_string()],
        "the duplicate threshold emits no second started event"
    );
}

#[tokio::test]
async fn consume_threshold_crossed_starts_an_sla() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: t0(),
        event: Event::ProposalThresholdCrossed {
            proposal: ProposalId::new(),
            cluster: ClusterId::new(),
            mandate,
            mandates: Vec::new(),
        },
    };

    let started = svc.consume(&envelope).await.expect("consume succeeds");
    assert_eq!(started.len(), 1, "evento antigo (sem lista) = 1 SLA");
    assert!(started[0].newly_started);
    assert_eq!(started[0].sla.mandate_id, mandate.as_uuid());

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consequence.sla.started".to_string()]
    );
}

#[tokio::test]
async fn consume_threshold_crossed_starts_one_sla_per_gabinete() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let m1 = seed_mandate(&db, org).await;
    let m2 = seed_mandate(&db, org).await;
    let m3 = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: t0(),
        event: Event::ProposalThresholdCrossed {
            proposal: ProposalId::new(),
            cluster: ClusterId::new(),
            mandate: m1,
            mandates: vec![m1, m2, m3],
        },
    };

    let started = svc.consume(&envelope).await.expect("consume succeeds");
    assert_eq!(started.len(), 3, "um SLA por gabinete");
    assert!(started.iter().all(|s| s.newly_started));
    let mandates: Vec<_> = started.iter().map(|s| s.sla.mandate_id).collect();
    assert_eq!(mandates, vec![m1.as_uuid(), m2.as_uuid(), m3.as_uuid()]);

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consequence.sla.started".to_string(); 3],
        "cada gabinete ganha seu próprio sla.started"
    );

    // Redelivery do MESMO evento: nenhum relógio novo, nenhuma emissão nova.
    let again = svc.consume(&envelope).await.expect("redelivery succeeds");
    assert_eq!(again.len(), 3);
    assert!(again.iter().all(|s| !s.newly_started));
    assert_eq!(outbox_event_types(&db, org).await.len(), 3);
}

#[tokio::test]
async fn consume_ignores_unrelated_events() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), t0());

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: t0(),
        event: Event::ConsensusClusterFormed {
            cluster: ClusterId::new(),
            size: 3,
        },
    };

    let result = svc.consume(&envelope).await.expect("consume succeeds");
    assert!(result.is_empty(), "non-threshold events are ignored");
    assert!(
        outbox_event_types(&db, org).await.is_empty(),
        "ignored events produce no SLA and no outbox emission"
    );
}

#[tokio::test]
async fn sweep_before_due_does_not_expire_and_is_idempotent() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    // Before due: nothing expires.
    assert_eq!(
        svc.sweep_expired(org, t0() + Duration::hours(10))
            .await
            .expect("early sweep"),
        0
    );
    assert_eq!(svc.get_sla(sla_id).await.unwrap().status, "pending");

    // Past due: expires exactly once; a second sweep is a no-op (idempotent).
    assert_eq!(
        svc.sweep_expired(org, t0() + Duration::hours(80))
            .await
            .expect("expiring sweep"),
        1
    );
    assert_eq!(
        svc.sweep_expired(org, t0() + Duration::hours(80))
            .await
            .expect("repeat sweep"),
        0,
        "re-running the sweep must not re-expire or re-emit"
    );

    // Only one expired event landed.
    let expired_events = outbox_event_types(&db, org)
        .await
        .into_iter()
        .filter(|t| t == "consequence.sla.expired")
        .count();
    assert_eq!(expired_events, 1);
}

#[tokio::test]
async fn respond_to_missing_sla_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone(), t0());

    let err = svc
        .respond(SlaId::new(), "olá", false)
        .await
        .expect_err("absent sla must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn respond_with_blank_body_is_rejected_before_any_write() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    let err = svc
        .respond(sla_id, "   ", false)
        .await
        .expect_err("blank body rejected");
    assert_eq!(err.code(), "invalid_input");

    // The SLA is untouched and no responded event was emitted.
    assert_eq!(svc.get_sla(sla_id).await.unwrap().status, "pending");
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consequence.sla.started".to_string()]
    );
}

#[tokio::test]
async fn responding_twice_conflicts_on_the_second_attempt() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    let started = svc
        .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
        .await
        .expect("start sla");
    let sla_id = SlaId::from_uuid(started.sla.id);

    svc.respond(sla_id, "Primeira resposta.", false)
        .await
        .expect("first response answers");

    let err = svc
        .respond(sla_id, "Segunda resposta.", true)
        .await
        .expect_err("a second response must conflict");
    assert_eq!(err.code(), "conflict");

    // Status stays Answered (the first decision is permanent).
    assert_eq!(svc.get_sla(sla_id).await.unwrap().status, "answered");
    assert_eq!(svc.outcome(sla_id).await.unwrap(), Some("answered".into()));
}

#[tokio::test]
async fn list_slas_keyset_pagination_round_trips() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone(), t0());

    // Start three SLAs (distinct proposals/clusters).
    let mut ids = Vec::new();
    for _ in 0..3 {
        let started = svc
            .start_sla(org, threshold(ProposalId::new(), ClusterId::new(), mandate))
            .await
            .expect("start sla");
        ids.push(started.sla.id);
    }
    ids.sort();

    let (page1, total) = svc.list_slas(org, None, 2).await.expect("page 1");
    assert_eq!(total, 3);
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].id, ids[0]);
    assert_eq!(page1[1].id, ids[1]);

    let cursor = SlaId::from_uuid(page1[1].id);
    let (page2, _) = svc.list_slas(org, Some(cursor), 2).await.expect("page 2");
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].id, ids[2]);
}

#[tokio::test]
async fn missing_sla_get_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone(), t0());
    let err = svc
        .get_sla(SlaId::new())
        .await
        .expect_err("absent sla must be NotFound");
    assert_eq!(err.code(), "not_found");
}
