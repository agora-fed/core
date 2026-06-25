//! Integration tests for `dsoc-notify` against a real PostgreSQL (TESTING.md).
//!
//! Each test seeds its own `org` + `citizen` with fresh UUIDv7 ids, so tests are
//! isolated without a shared transaction. Time is injected via a fixed clock. Delivery
//! signals are emitted through the transactional outbox (ADR-0006), so they are asserted by
//! reading `events_log` rather than a bus double. No `sleep`, no real SMTP.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::error::{Error, Result};
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, ClusterId, EventId, MandateId, OrgId, ProposalId, SlaId};
use dsoc_core::traits::{Authorization, VerificationLevel};
use dsoc_db::Db;
use dsoc_notify::domain::MAX_DELIVERY_ATTEMPTS;
use dsoc_notify::service::{Channel, ChannelError, OutboundMessage};
use dsoc_notify::{NoopChannel, NotifyService};

/// Deterministic clock: SLA logic must never depend on wall time.
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// Authorization stub resolving every citizen to a fixed verification level.
#[derive(Debug)]
struct StubAuthz {
    level: VerificationLevel,
}

#[async_trait]
impl Authorization for StubAuthz {
    async fn level(&self, _org: OrgId, _citizen: CitizenId) -> Result<VerificationLevel> {
        Ok(self.level)
    }

    async fn require(
        &self,
        _org: OrgId,
        _citizen: CitizenId,
        required: VerificationLevel,
    ) -> Result<()> {
        if self.level >= required {
            Ok(())
        } else {
            Err(Error::Forbidden("insufficient verification".to_owned()))
        }
    }
}

/// A channel double that always rejects, to exercise the bounded-retry path.
#[derive(Debug)]
struct FailingChannel;
#[async_trait]
impl Channel for FailingChannel {
    async fn send(&self, _message: &OutboundMessage) -> std::result::Result<(), ChannelError> {
        Err(ChannelError("transport unavailable".into()))
    }
}

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap()
}

async fn pool() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to postgres")
}

/// Seed a fresh org + citizen and return their ids.
async fn seed_identity(db: &Db, at: DateTime<Utc>) -> (OrgId, CitizenId) {
    let org = OrgId::new();
    let citizen = CitizenId::new();
    sqlx::query!(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)",
        org.as_uuid(),
        format!("org-{}", org.as_uuid()),
        "Test Org",
        at,
    )
    .execute(db)
    .await
    .expect("seed org");
    sqlx::query!(
        "INSERT INTO citizen (id, org_id, oidc_subject, created_at) VALUES ($1, $2, $3, $4)",
        citizen.as_uuid(),
        org.as_uuid(),
        format!("sub-{}", citizen.as_uuid()),
        at,
    )
    .execute(db)
    .await
    .expect("seed citizen");
    (org, citizen)
}

/// Count the cross-crate events of a given type emitted for `org` via the outbox (`events_log`).
async fn count_events(db: &Db, org: OrgId, event_type: &str) -> i64 {
    sqlx::query!(
        r#"SELECT COUNT(*) AS "c!" FROM events_log WHERE org_id = $1 AND event_type = $2"#,
        org.as_uuid(),
        event_type,
    )
    .fetch_one(db)
    .await
    .expect("count events")
    .c
}

fn service(db: Db) -> NotifyService {
    service_with(db, VerificationLevel::Email)
}

fn service_with(db: Db, level: VerificationLevel) -> NotifyService {
    NotifyService::new(
        db,
        Arc::new(FixedClock(fixed_time())),
        Arc::new(StubAuthz { level }),
    )
}

fn sla_started_envelope(org: OrgId, at: DateTime<Utc>) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event: Event::ConsequenceSlaStarted {
            sla: SlaId::new(),
            mandate: MandateId::new(),
            cluster: ClusterId::new(),
            due: at,
        },
    }
}

#[tokio::test]
async fn consuming_sla_started_enqueues_outbox_row() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db.clone());

    let envelope = sla_started_envelope(org, at);
    let id = svc
        .enqueue_for_event(&envelope, citizen)
        .await
        .expect("enqueue")
        .expect("sla.started maps to a notification");

    let row = dsoc_notify::queries::fetch_outbox(&db, id.as_uuid())
        .await
        .expect("row exists");
    assert_eq!(row.status, "pending");
    assert_eq!(row.channel, "email");
    assert_eq!(row.citizen_id, citizen.as_uuid());
    // Enqueue alone emits nothing; emission happens at dispatch.
    assert_eq!(count_events(&db, org, "notify.dispatched").await, 0);
    assert_eq!(count_events(&db, org, "notify.delivery.failed").await, 0);
}

#[tokio::test]
async fn unhandled_event_enqueues_nothing() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db.clone());

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event: Event::VoteCast {
            vote: dsoc_core::ids::VoteId::new(),
            proposal: ProposalId::new(),
        },
    };
    let result = svc.enqueue_for_event(&envelope, citizen).await.expect("ok");
    assert!(result.is_none());
}

#[tokio::test]
async fn dispatch_success_emits_dispatched_and_writes_receipt() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db.clone());

    let id = svc
        .enqueue_for_event(&sla_started_envelope(org, at), citizen)
        .await
        .expect("enqueue")
        .expect("some id");

    let report = svc.dispatch(&NoopChannel, id).await.expect("dispatch");
    assert!(report.accepted);
    assert_eq!(report.attempts, 1);

    // Exactly one receipt, and a single notify.dispatched event committed to the outbox.
    let receipts = dsoc_notify::queries::count_receipts(&db, id.as_uuid())
        .await
        .expect("count");
    assert_eq!(receipts, 1);
    assert_eq!(count_events(&db, org, "notify.dispatched").await, 1);
    assert_eq!(count_events(&db, org, "notify.delivery.failed").await, 0);

    // The row is now terminal; re-dispatch is rejected (idempotent, bounded).
    let err = svc.dispatch(&NoopChannel, id).await.unwrap_err();
    assert_eq!(err.code(), "conflict");
    // Re-dispatch did not emit a second event.
    assert_eq!(count_events(&db, org, "notify.dispatched").await, 1);
}

#[tokio::test]
async fn failure_path_is_bounded_and_emits_delivery_failed() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db.clone());

    let id = svc
        .enqueue_for_event(&sla_started_envelope(org, at), citizen)
        .await
        .expect("enqueue")
        .expect("some id");

    // Every attempt below the cap stays pending and emits nothing.
    for attempt in 1..MAX_DELIVERY_ATTEMPTS {
        let report = svc.dispatch(&FailingChannel, id).await.expect("dispatch");
        assert!(!report.accepted);
        assert_eq!(report.attempts, attempt);
        assert_eq!(report.status.as_str(), "pending");
        assert_eq!(
            count_events(&db, org, "notify.delivery.failed").await,
            0,
            "no event before retries are exhausted"
        );
    }

    // The attempt that reaches the cap fails permanently and emits exactly once.
    let last = svc.dispatch(&FailingChannel, id).await.expect("dispatch");
    assert_eq!(last.attempts, MAX_DELIVERY_ATTEMPTS);
    assert_eq!(last.status.as_str(), "failed");

    assert_eq!(count_events(&db, org, "notify.delivery.failed").await, 1);
    assert_eq!(count_events(&db, org, "notify.dispatched").await, 0);

    // One receipt per attempt; retries are bounded by MAX_DELIVERY_ATTEMPTS.
    let receipts = dsoc_notify::queries::count_receipts(&db, id.as_uuid())
        .await
        .expect("count");
    assert_eq!(receipts, i64::from(MAX_DELIVERY_ATTEMPTS));

    // Terminal: further dispatch is rejected.
    let err = svc.dispatch(&FailingChannel, id).await.unwrap_err();
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn dispatch_unknown_notification_is_not_found() {
    let db = pool().await;
    let svc = service(db);
    let missing = dsoc_core::ids::NotificationId::new();
    let err = svc.dispatch(&NoopChannel, missing).await.unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn register_device_token_persists_and_is_idempotent() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db.clone());

    let first = svc
        .register_device_token(org, citizen, "ios", "device-token-xyz")
        .await
        .expect("register");

    // Re-registering the same triple returns the REAL stored id (not a fresh phantom id) and
    // does not create a second row.
    let second = svc
        .register_device_token(org, citizen, "ios", "device-token-xyz")
        .await
        .expect("idempotent re-register");
    assert_eq!(
        first, second,
        "idempotent re-register must return the stored id"
    );

    // The returned id resolves to the actual persisted row (guards against phantom ids).
    let stored = sqlx::query!(
        "SELECT id FROM notify_device_token WHERE citizen_id = $1",
        citizen.as_uuid(),
    )
    .fetch_all(&db)
    .await
    .expect("load tokens");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, first);

    let count = sqlx::query!(
        "SELECT COUNT(*) AS \"c!\" FROM notify_device_token WHERE citizen_id = $1",
        citizen.as_uuid(),
    )
    .fetch_one(&db)
    .await
    .expect("count tokens");
    assert_eq!(count.c, 1);
    assert_ne!(first, uuid::Uuid::nil());
}

#[tokio::test]
async fn register_device_token_rejects_bad_platform() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    let svc = service(db);

    let err = svc
        .register_device_token(org, citizen, "pager", "tok")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn register_device_token_requires_email_verification() {
    let db = pool().await;
    let at = fixed_time();
    let (org, citizen) = seed_identity(&db, at).await;
    // An anonymous (unverified) caller may not register push tokens.
    let svc = service_with(db.clone(), VerificationLevel::Anonymous);

    let err = svc
        .register_device_token(org, citizen, "ios", "device-token-xyz")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");

    // Nothing was persisted for the rejected caller.
    let count = sqlx::query!(
        "SELECT COUNT(*) AS \"c!\" FROM notify_device_token WHERE citizen_id = $1",
        citizen.as_uuid(),
    )
    .fetch_one(&db)
    .await
    .expect("count tokens");
    assert_eq!(count.c, 0);
}
