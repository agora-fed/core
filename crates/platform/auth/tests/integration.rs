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

#[tokio::test]
async fn register_then_login_with_cpf_and_password() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);

    let email = format!("cidadao-{}@exemplo.br", Uuid::now_v7());
    let s = auth
        .register(org, &email, "senha-super-secreta", "529.982.247-25")
        .await
        .expect("register");

    // credential stored: normalized CPF + algorithmic status; password Argon2id-hashed, never plain.
    let (cpf, status): (String, String) =
        sqlx::query_as("SELECT cpf, cpf_status FROM auth_credential WHERE citizen_id = $1")
            .bind(s.citizen.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(cpf, "52998224725");
    assert_eq!(status, "validated");
    let hash: String =
        sqlx::query_scalar("SELECT password_hash FROM auth_credential WHERE citizen_id = $1")
            .bind(s.citizen.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(hash.starts_with("$argon2"));
    assert!(!hash.contains("senha-super-secreta"));

    // login: same citizen, fresh distinct session.
    let s2 = auth
        .login(org, &email, "senha-super-secreta")
        .await
        .expect("login");
    assert_eq!(s2.citizen, s.citizen);
    assert_ne!(s2.id, s.id);
}

#[tokio::test]
async fn login_with_wrong_password_is_unauthorized() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);
    let email = format!("x-{}@exemplo.br", Uuid::now_v7());
    auth.register(org, &email, "senha-correta-123", "529.982.247-25")
        .await
        .expect("register");
    let err = auth.login(org, &email, "senha-errada").await.unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Unauthorized));
}

#[tokio::test]
async fn duplicate_cpf_is_conflict_and_rolls_back() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);
    let email = format!("dup-{}@exemplo.br", Uuid::now_v7());
    auth.register(org, &email, "senha-correta-123", "529.982.247-25")
        .await
        .expect("first");
    let other = format!("dup2-{}@exemplo.br", Uuid::now_v7());
    let err = auth
        .register(org, &other, "senha-correta-123", "529.982.247-25")
        .await
        .unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Conflict(_)));
}

#[tokio::test]
async fn invalid_cpf_is_rejected() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let bus = Arc::new(RecordingEventBus::new());
    let auth = build_auth(db.clone(), clock, bus as Arc<dyn EventBus>);
    let email = format!("bad-{}@exemplo.br", Uuid::now_v7());
    let err = auth
        .register(org, &email, "senha-correta-123", "111.111.111-11")
        .await
        .unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Validation(_)));
}

// -----------------------------------------------------------------------------
// signup_verify (0.25.0-fediverso-verify): request grava pending; confirm
// materializa citizen+credential+session numa única tx.
// -----------------------------------------------------------------------------

/// Extrai o token plaintext do pending row lendo a chamada mais recente ao
/// serviço. Como o token é hasheado em repouso, o teste não pode
/// "roubar" o plaintext do banco — em vez disso, injeta um token conhecido
/// via SQL bruto no `auth_pending_signup`, cortando o `request` do fluxo e
/// exercendo apenas o `confirm` (que é a parte crítica: materialização
/// transacional).
#[tokio::test]
async fn signup_verify_confirm_materializes_citizen_and_session() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let svc = dsoc_auth::signup_verify::SignupVerifyService::new_for_tests(
        db.clone(),
        clock.clone(),
        "https://test.local",
        3600,
        3600,
    );

    // Gera um token e insere um pending manualmente (equivalente a request_cidadao,
    // mas sem tocar em SMTP).
    // 32 bytes ~ o mesmo tamanho de token do serviço em prod.
    let token: String = URL_SAFE_NO_PAD.encode([7u8; 32]);
    let hash: Vec<u8> = {
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        h.finalize().to_vec()
    };
    let pending_id = Uuid::now_v7();
    let email = format!("verify-{}@exemplo.br", Uuid::now_v7());
    let expires_at = now() + chrono::Duration::hours(1);
    let password_hash = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$hash".to_owned();
    sqlx::query(
        r"INSERT INTO auth_pending_signup
            (id, org_id, email, password_hash, cpf, role, mandate_id,
             token_hash, expires_at, used_at, request_ip, created_at)
          VALUES ($1, $2, $3, $4, $5, 'cidadao', NULL,
                  $6, $7, NULL, NULL, $8)",
    )
    .bind(pending_id)
    .bind(org.as_uuid())
    .bind(&email)
    .bind(&password_hash)
    .bind("52998224725")
    .bind(&hash)
    .bind(expires_at)
    .bind(now())
    .execute(&db)
    .await
    .expect("seed pending");

    let session = svc.confirm(&token).await.expect("confirm");

    // Row pending marcada como usada.
    let used_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT used_at FROM auth_pending_signup WHERE id = $1")
            .bind(pending_id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(used_at.is_some(), "pending marcada como usada dentro da tx");

    // Credential/citizen materializados com o hash exato do pending (nunca
    // rehashed no confirm) e o e-mail normalizado.
    let (cred_email, cred_hash, cred_cpf): (String, String, String) = sqlx::query_as(
        "SELECT email, password_hash, cpf FROM auth_credential WHERE citizen_id = $1",
    )
    .bind(session.citizen.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(cred_email, email);
    assert_eq!(cred_hash, password_hash);
    assert_eq!(cred_cpf, "52998224725");

    // Session emitida com o public_handle esperado (formato @cidadao-<curto>).
    assert!(!session.public_handle.is_empty());
    let live: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth_session WHERE id = $1 AND expires_at > $2")
            .bind(session.id)
            .bind(now())
            .fetch_optional(&db)
            .await
            .unwrap();
    assert_eq!(live, Some(session.id), "sessão viva depois do commit");

    // Retry do mesmo token deve falhar (single-use, agora used_at NOT NULL).
    let err = svc.confirm(&token).await.unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Unauthorized));
}

#[tokio::test]
async fn signup_verify_confirm_rejects_expired_token() {
    use sha2::{Digest, Sha256};

    let db = connect().await;
    let org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let svc = dsoc_auth::signup_verify::SignupVerifyService::new_for_tests(
        db.clone(),
        clock.clone(),
        "https://test.local",
        3600,
        3600,
    );
    let token = format!("expired-{}", Uuid::now_v7());
    let hash: Vec<u8> = {
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        h.finalize().to_vec()
    };
    // Já vencido no momento da consulta (expires_at = 1h ANTES do relógio fixado).
    let expires_at = now() - chrono::Duration::hours(1);
    sqlx::query(
        r"INSERT INTO auth_pending_signup
            (id, org_id, email, password_hash, cpf, role, mandate_id,
             token_hash, expires_at, used_at, request_ip, created_at)
          VALUES ($1, $2, $3, $4, $5, 'cidadao', NULL, $6, $7, NULL, NULL, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(org.as_uuid())
    .bind(format!("exp-{}@exemplo.br", Uuid::now_v7()))
    .bind("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$hash")
    .bind("52998224725")
    .bind(&hash)
    .bind(expires_at)
    .bind(now())
    .execute(&db)
    .await
    .expect("seed pending");

    let err = svc.confirm(&token).await.unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Unauthorized));
}

#[tokio::test]
async fn signup_verify_confirm_rejects_unknown_token() {
    let db = connect().await;
    let _org = seed_org(&db).await;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let svc = dsoc_auth::signup_verify::SignupVerifyService::new_for_tests(
        db.clone(),
        clock,
        "https://test.local",
        3600,
        3600,
    );
    let err = svc.confirm("no-such-token").await.unwrap_err();
    assert!(matches!(err, dsoc_core::Error::Unauthorized));
}
