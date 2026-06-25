//! Integration tests for `dsoc-auth` against a real PostgreSQL (TESTING.md: no mocked DB).
//! Each test isolates its data behind a unique `OrgId` and OIDC subject, uses a deterministic
//! `FixedClock` (never sleeps), and asserts events via `dsoc_core::testing::RecordingEventBus`.
//!
//! Requires `DATABASE_URL` and the `0100_auth_*` migration applied (the harness does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use uuid::Uuid;

use dsoc_auth::{StaticKeySource, TokenValidator, ZitadelAuth};
use dsoc_core::events::Event;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::{Authorization, Clock, Error, EventBus, VerificationLevel};
use dsoc_db::Db;

const TEST_PRIV_DER: &[u8] = include_bytes!("data/test_priv.der");
const TEST_PUB_DER: &[u8] = include_bytes!("data/test_pub.der");
const ISSUER: &str = "https://id.democracia.social.example";

/// Deterministic clock — SLA/session timing is reproducible without sleeping.
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Serialize)]
struct Claims {
    sub: String,
    iss: String,
    exp: i64,
    iat: i64,
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn sign(sub: &str, exp: i64) -> String {
    let key = EncodingKey::from_rsa_der(TEST_PRIV_DER);
    let claims = Claims {
        sub: sub.to_owned(),
        iss: ISSUER.to_owned(),
        exp,
        iat: now().timestamp(),
    };
    encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap()
}

async fn connect() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to Postgres")
}

/// Insert a fresh isolated org and return its id.
async fn seed_org(db: &Db) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid().simple()))
        .bind("Test Org")
        .bind(now())
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Insert a citizen directly (for tests that need an existing identity).
async fn seed_citizen(db: &Db, org: OrgId, subject: &str, level: VerificationLevel) -> CitizenId {
    let citizen = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(citizen.as_uuid())
    .bind(org.as_uuid())
    .bind(subject)
    .bind(dsoc_auth::domain::level_as_str(level))
    .bind(now())
    .execute(db)
    .await
    .expect("seed citizen");
    citizen
}

fn build_auth(db: Db, clock: Arc<dyn Clock>, bus: Arc<dyn EventBus>) -> ZitadelAuth {
    let source = StaticKeySource::from_rsa_der(TEST_PUB_DER, ISSUER, None).unwrap();
    let validator = TokenValidator::new(Arc::new(source));
    ZitadelAuth::new(db, clock, bus, validator, 3600)
}

#[tokio::test]
async fn valid_token_issues_a_session_and_provisions_citizen() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus.clone() as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let token = sign(&subject, now().timestamp() + 3600);

    let session = auth.create_session(org, &token).await.expect("session");
    assert_eq!(session.issued_at, now());
    assert_eq!(session.expires_at, now() + chrono::Duration::seconds(3600));
    assert!(session.public_handle.starts_with("u-"));

    // Citizen provisioned at Email level.
    let level: String = sqlx::query_scalar("SELECT verification_level FROM citizen WHERE id = $1")
        .bind(session.citizen.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(level, "email");

    // Exactly one session row persisted.
    let session_count: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_session WHERE id = $1")
        .bind(session.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(session_count, 1);

    // No upgrade event on first provisioning.
    assert_eq!(bus.count(), 0);
}

#[tokio::test]
async fn expired_token_is_rejected_unauthorized() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db, clock, bus as Arc<dyn EventBus>);

    let token = sign("expired-subject", now().timestamp() - 1);
    let err = auth.create_session(org, &token).await.unwrap_err();
    assert!(matches!(err, Error::Unauthorized));
}

#[tokio::test]
async fn garbage_token_is_rejected_unauthorized() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db, clock, bus as Arc<dyn EventBus>);

    let err = auth
        .create_session(org, "this.is.not.a.jwt")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unauthorized));
}

#[tokio::test]
async fn require_enforces_minimum_verification_level() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let citizen = seed_citizen(&db, org, &subject, VerificationLevel::Email).await;

    assert_eq!(
        auth.level(org, citizen).await.unwrap(),
        VerificationLevel::Email
    );
    auth.require(org, citizen, VerificationLevel::Email)
        .await
        .expect("email satisfies email");
    let err = auth
        .require(org, citizen, VerificationLevel::Directory)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));
}

#[tokio::test]
async fn level_lookup_for_unknown_citizen_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db, clock, bus as Arc<dyn EventBus>);

    let err = auth.level(org, CitizenId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn upgrade_raises_level_audits_and_emits_event() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus.clone() as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let citizen = seed_citizen(&db, org, &subject, VerificationLevel::Email).await;

    let new_level = auth
        .upgrade_level(org, citizen, VerificationLevel::Directory)
        .await
        .expect("upgrade");
    assert_eq!(new_level, VerificationLevel::Directory);

    assert_eq!(
        auth.level(org, citizen).await.unwrap(),
        VerificationLevel::Directory
    );

    // ADR-0006: the event is emitted through the transactional outbox, not the fire-and-forget
    // bus — so nothing reaches the bus, and exactly one row lands in `events_log`.
    assert_eq!(bus.count(), 0);
    let payload: String = sqlx::query_scalar(
        "SELECT payload::text FROM events_log \
         WHERE org_id = $1 AND event_type = 'auth.verification.upgraded'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    match serde_json::from_str::<Event>(&payload).unwrap() {
        Event::AuthVerificationUpgraded { citizen: c } => assert_eq!(c, citizen),
        other => panic!("unexpected event: {other:?}"),
    }

    // Audit trail recorded the transition.
    let history = auth
        .verification_history(citizen, None, 10)
        .await
        .expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].old_level, VerificationLevel::Email);
    assert_eq!(history[0].new_level, VerificationLevel::Directory);
}

#[tokio::test]
async fn non_upgrade_is_silent_no_event() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus.clone() as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let citizen = seed_citizen(&db, org, &subject, VerificationLevel::Directory).await;

    // Downgrade attempt: no change, no event (neither on the bus nor in the outbox).
    let result = auth
        .upgrade_level(org, citizen, VerificationLevel::Email)
        .await
        .expect("no-op");
    assert_eq!(result, VerificationLevel::Directory);
    assert_eq!(bus.count(), 0);

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events_log \
         WHERE org_id = $1 AND event_type = 'auth.verification.upgraded'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(event_count, 0);

    let history = auth
        .verification_history(citizen, None, 10)
        .await
        .expect("history");
    assert!(history.is_empty());
}

#[tokio::test]
async fn me_returns_identity_after_session() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db, clock, bus as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let token = sign(&subject, now().timestamp() + 3600);
    let session = auth.create_session(org, &token).await.expect("session");

    let identity = auth.me(org, &token).await.expect("me");
    assert_eq!(identity.citizen, session.citizen);
    assert_eq!(identity.oidc_subject, subject);
    assert_eq!(identity.level, VerificationLevel::Email);
    assert_eq!(identity.public_handle, session.public_handle);
}

#[tokio::test]
async fn second_login_reuses_citizen_and_issues_fresh_session() {
    // Returning-user branch: `find_citizen_by_subject` returns `Some`, so the second login skips
    // the citizen insert and issues a fresh session for the existing citizen. A regression here
    // would surface the unique-violation as `Error::Conflict` for every returning user.
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);

    let subject = format!("sub-{}", Uuid::now_v7());
    let token = sign(&subject, now().timestamp() + 3600);

    let first = auth.create_session(org, &token).await.expect("first login");
    let second = auth
        .create_session(org, &token)
        .await
        .expect("second login must not Conflict");

    // Same real stored citizen, never a phantom id; a fresh session each time.
    assert_eq!(first.citizen, second.citizen);
    assert_ne!(first.id, second.id);

    // Exactly one citizen row for the subject (the insert was skipped on the second login).
    let citizen_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM citizen WHERE org_id = $1 AND oidc_subject = $2")
            .bind(org.as_uuid())
            .bind(&subject)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(citizen_count, 1);

    // Two distinct session rows for that citizen.
    let session_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_session WHERE citizen_id = $1")
            .bind(second.citizen.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(session_count, 2);
}
