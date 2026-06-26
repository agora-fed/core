//! Integration tests for `dsoc-initiatives` against a real PostgreSQL (docs/TESTING.md: no mocked
//! DB). Each test isolates its data behind a freshly generated `OrgId`, injects a deterministic
//! `FixedClock` (never sleeps), and authorizes through a stub `Authorization`. The crate emits no
//! cross-crate events, so the `RecordingEventBus` stays empty by design.
//!
//! Requires `DATABASE_URL` and the baseline + `0230_initiatives_core.sql` applied (the harness
//! does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::traits::Space;
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;
use dsoc_initiatives::{InitiativeService, InitiativesSpace};

/// Deterministic clock — escalation timing is reproducible without sleeping.
#[derive(Debug, Clone, Copy)]
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

#[async_trait::async_trait]
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

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

async fn connect() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to Postgres")
}

/// Build a service at a given clock instant and caller verification level.
fn service(
    db: Db,
    at: DateTime<Utc>,
    level: VerificationLevel,
) -> (InitiativeService, Arc<RecordingEventBus>) {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(at));
    let bus = Arc::new(RecordingEventBus::new());
    let authz = Arc::new(StubAuthz { level });
    let svc = InitiativeService::new(db, clock, bus.clone() as Arc<dyn EventBus>, authz);
    (svc, bus)
}

/// Insert a fresh isolated org and return its id.
async fn seed_org(db: &Db) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid().simple()))
        .bind("Prefeitura Teste")
        .bind(now())
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Insert a fresh citizen in an org (signatures FK-reference `citizen`).
async fn seed_citizen(db: &Db, org: OrgId) -> CitizenId {
    let citizen = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, 'email', $4)",
    )
    .bind(citizen.as_uuid())
    .bind(org.as_uuid())
    .bind(format!("sub-{}", citizen.as_uuid().simple()))
    .bind(now())
    .execute(db)
    .await
    .expect("seed citizen");
    citizen
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_initiative_with_zero_count() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let (svc, bus) = service(db.clone(), now(), VerificationLevel::Email);

    let initiative = svc
        .create(org, author, "  Mais ciclovias  ", "Body texto", 100)
        .await
        .expect("create");

    assert_eq!(initiative.title, "Mais ciclovias", "title is trimmed");
    assert_eq!(initiative.threshold, 100);
    assert_eq!(initiative.signature_count, 0);
    assert!(!initiative.has_reached());
    assert_eq!(initiative.created_at, now());

    // Persisted with a zero tally and NULL reached_at.
    let (count, reached): (i64, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT signature_count, reached_at FROM initiative WHERE id = $1")
            .bind(initiative.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(reached, None);
    // No cross-crate events emitted.
    assert_eq!(bus.count(), 0);
}

#[tokio::test]
async fn create_requires_email_level_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    // Anonymous is below the Email bar for mutations.
    let (svc, _bus) = service(db.clone(), now(), VerificationLevel::Anonymous);

    let err = svc
        .create(org, author, "Titulo", "Corpo", 10)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));

    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM initiative WHERE org_id = $1")
        .bind(org.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(total, 0, "nothing persisted when forbidden");
}

#[tokio::test]
async fn create_rejects_blank_title_and_bad_threshold() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    let blank = svc
        .create(org, author, "   ", "Corpo", 10)
        .await
        .unwrap_err();
    assert!(matches!(blank, Error::Validation(_)));

    let bad_threshold = svc
        .create(org, author, "Titulo", "Corpo", 0)
        .await
        .unwrap_err();
    assert!(matches!(bad_threshold, Error::Validation(_)));
}

// ---------------------------------------------------------------------------
// sign
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sign_increments_count_and_records_signature() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let signer = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db.clone(), now(), VerificationLevel::Email);

    let initiative = svc
        .create(org, author, "Titulo", "Corpo", 5)
        .await
        .expect("create");

    let outcome = svc.sign(org, signer, initiative.id).await.expect("sign");
    assert!(outcome.newly_signed);
    assert_eq!(outcome.signature_count, 1);
    assert!(!outcome.has_reached());

    // The tally on the row advanced, and the signature row exists.
    let count: i64 = sqlx::query_scalar("SELECT signature_count FROM initiative WHERE id = $1")
        .bind(initiative.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let sig_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM initiative_signature WHERE initiative_id = $1 AND citizen_id = $2",
    )
    .bind(initiative.id)
    .bind(signer.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(sig_count, 1);
}

#[tokio::test]
async fn sign_is_idempotent_for_the_same_citizen() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let signer = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db.clone(), now(), VerificationLevel::Email);

    let initiative = svc
        .create(org, author, "Titulo", "Corpo", 5)
        .await
        .expect("create");

    let first = svc.sign(org, signer, initiative.id).await.expect("sign 1");
    let second = svc.sign(org, signer, initiative.id).await.expect("sign 2");

    assert!(first.newly_signed);
    assert!(!second.newly_signed, "repeat sign is a no-op");
    assert_eq!(second.signature_count, 1, "tally not double-counted");
    // The same real signature id is returned both times (re-select, not a phantom).
    assert_eq!(first.signature_id, second.signature_id);

    let count: i64 = sqlx::query_scalar("SELECT signature_count FROM initiative WHERE id = $1")
        .bind(initiative.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM initiative_signature WHERE initiative_id = $1")
            .bind(initiative.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(rows, 1, "exactly one signature row for the citizen");
}

#[tokio::test]
async fn crossing_threshold_sets_reached_at_exactly_once() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let c1 = seed_citizen(&db, org).await;
    let c2 = seed_citizen(&db, org).await;
    let c3 = seed_citizen(&db, org).await;
    // threshold = 2: the second distinct signature crosses it.
    let (svc, _bus) = service(db.clone(), now(), VerificationLevel::Email);

    let initiative = svc
        .create(org, author, "Titulo", "Corpo", 2)
        .await
        .expect("create");

    let first = svc.sign(org, c1, initiative.id).await.expect("sign 1");
    assert!(!first.has_reached(), "below threshold after one signature");

    let second = svc.sign(org, c2, initiative.id).await.expect("sign 2");
    assert!(second.has_reached(), "threshold crossed on the second sign");
    assert_eq!(second.reached_at, Some(now()));
    assert_eq!(second.signature_count, 2);

    // A third signature must NOT move reached_at (one-shot marker).
    let third = svc.sign(org, c3, initiative.id).await.expect("sign 3");
    assert_eq!(third.signature_count, 3);
    assert_eq!(
        third.reached_at,
        Some(now()),
        "reached_at is unchanged by later signatures"
    );

    let reached_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT reached_at FROM initiative WHERE id = $1")
            .bind(initiative.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(reached_at, Some(now()));
}

#[tokio::test]
async fn sign_requires_email_level_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let signer = seed_citizen(&db, org).await;

    // Author at Email creates; signer attempt is made by an Anonymous-level service.
    let (creator, _b) = service(db.clone(), now(), VerificationLevel::Email);
    let initiative = creator
        .create(org, author, "Titulo", "Corpo", 5)
        .await
        .expect("create");

    let (anon_svc, _bus) = service(db.clone(), now(), VerificationLevel::Anonymous);
    let err = anon_svc.sign(org, signer, initiative.id).await.unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));

    let count: i64 = sqlx::query_scalar("SELECT signature_count FROM initiative WHERE id = $1")
        .bind(initiative.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0, "no signature recorded when forbidden");
}

#[tokio::test]
async fn sign_unknown_initiative_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let signer = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    let err = svc.sign(org, signer, Uuid::now_v7()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn sign_in_wrong_org_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let other_org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let signer = seed_citizen(&db, other_org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    let initiative = svc
        .create(org, author, "Titulo", "Corpo", 5)
        .await
        .expect("create");

    // Same initiative id, but scoped to a different org -> NotFound.
    let err = svc
        .sign(other_org, signer, initiative.id)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// get / list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_returns_initiative_and_unknown_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    let created = svc
        .create(org, author, "Titulo", "Corpo", 5)
        .await
        .expect("create");
    let fetched = svc.get(org, created.id).await.expect("get");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.title, "Titulo");

    let err = svc.get(org, Uuid::now_v7()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn list_paginates_by_keyset() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    // Create three initiatives (ids are time-ordered UUIDv7, so creation order == id order).
    let a = svc.create(org, author, "A", "Corpo", 5).await.expect("a");
    let b = svc.create(org, author, "B", "Corpo", 5).await.expect("b");
    let c = svc.create(org, author, "C", "Corpo", 5).await.expect("c");

    // First page of 2.
    let page1 = svc.list(org, None, Some(2)).await.expect("page1");
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].id, a.id);
    assert_eq!(page1[1].id, b.id);

    // Second page using the last id of page1 as the cursor.
    let page2 = svc
        .list(org, Some(page1[1].id), Some(2))
        .await
        .expect("page2");
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].id, c.id);
}

#[tokio::test]
async fn list_is_scoped_to_org() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let other = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let (svc, _bus) = service(db, now(), VerificationLevel::Email);

    svc.create(org, author, "Mine", "Corpo", 5)
        .await
        .expect("mine");

    let others = svc.list(other, None, None).await.expect("list other");
    assert!(others.is_empty(), "other org sees no initiatives");
    let mine = svc.list(org, None, None).await.expect("list mine");
    assert_eq!(mine.len(), 1);
}

// ---------------------------------------------------------------------------
// Space trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn space_gates_components_and_kind() {
    let db = connect().await;
    let space = InitiativesSpace::new(db);
    assert_eq!(space.kind(), "initiatives");
    assert!(space.allows_component("proposals"));
    assert!(space.allows_component("comments"));
    assert!(!space.allows_component("budgets"));
    assert!(!space.allows_component("meetings"));
}

#[tokio::test]
async fn space_ensure_open_checks_org_existence() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let space = InitiativesSpace::new(db);

    space
        .ensure_open(org, SpaceId::new())
        .await
        .expect("existing org is open");

    let err = space
        .ensure_open(OrgId::new(), SpaceId::new())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}
