//! Integration tests for `dsoc-assemblies` against a real PostgreSQL (docs/TESTING.md: no mocked
//! DB). Each test isolates its data behind a freshly generated `OrgId`/assembly id, injects a
//! deterministic `FixedClock` (never sleeps), and exercises the service end-to-end: authorization,
//! validation, idempotent membership, and keyset pagination.
//!
//! Requires `DATABASE_URL` and the baseline + `0220_assemblies_membership.sql` applied.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};

use dsoc_assemblies::domain;
use dsoc_assemblies::{AssembliesService, AssembliesSpace};
use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;

/// Deterministic clock — timing is reproducible without sleeping.
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
fn service(db: Db, level: VerificationLevel) -> AssembliesService {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let authz: Arc<dyn Authorization> = Arc::new(StubAuthz { level });
    AssembliesService::new(db, clock, authz)
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

/// Insert a fresh citizen in an org and return its id (needed for the `assembly_member` FK).
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
// create_assembly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_assembly_persists_and_returns_real_row() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);

    let assembly = svc
        .create_assembly(org, CitizenId::new(), "  Assembleia de Saúde  ")
        .await
        .expect("create");
    assert_eq!(assembly.name, "Assembleia de Saúde", "name is trimmed");
    assert_eq!(assembly.org, org);
    assert_eq!(assembly.created_at, now());

    let name: String = sqlx::query_scalar("SELECT name FROM assembly WHERE id = $1")
        .bind(assembly.id.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(name, "Assembleia de Saúde");
}

#[tokio::test]
async fn create_assembly_requires_email_level_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    // Anonymous caller is below the Email bar for mutations.
    let svc = service(db.clone(), VerificationLevel::Anonymous);

    let err = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM assembly WHERE org_id = $1")
        .bind(org.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0, "no assembly persisted on forbidden");
}

#[tokio::test]
async fn create_assembly_rejects_blank_name() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db, VerificationLevel::Email);
    let err = svc
        .create_assembly(org, CitizenId::new(), "   ")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn get_assembly_roundtrips_and_unknown_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db, VerificationLevel::Email);

    let created = svc
        .create_assembly(org, CitizenId::new(), "Conselho")
        .await
        .expect("create");
    let fetched = svc.get_assembly(org, created.id).await.expect("get");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.name, "Conselho");

    let err = svc.get_assembly(org, SpaceId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// add_member
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_member_persists_and_lists() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");
    let citizen = seed_citizen(&db, org).await;

    let member = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "chair")
        .await
        .expect("add member");
    assert_eq!(member.assembly, assembly.id);
    assert_eq!(member.citizen, citizen);
    assert_eq!(member.role, "chair");
    assert_eq!(member.joined_at, now());

    let roster = svc
        .list_members(assembly.id, None, Some(10))
        .await
        .expect("list");
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].id, member.id);
}

#[tokio::test]
async fn add_member_defaults_blank_role_to_member() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");
    let citizen = seed_citizen(&db, org).await;

    let member = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "")
        .await
        .expect("add member");
    assert_eq!(member.role, "member");
}

#[tokio::test]
async fn add_member_is_idempotent_returning_existing_row() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");
    let citizen = seed_citizen(&db, org).await;

    let first = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "chair")
        .await
        .expect("add member 1");
    // Re-add the same citizen (even with a different requested role): same row, no duplicate, and
    // the original role is preserved (idempotent no-op insert).
    let second = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "observer")
        .await
        .expect("add member 2");
    assert_eq!(second.id, first.id, "same stored membership id");
    assert_eq!(second.role, "chair", "original role preserved");

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM assembly_member WHERE assembly_id = $1")
            .bind(assembly.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1, "no duplicate membership");
}

#[tokio::test]
async fn add_member_requires_authorized_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc_ok = service(db.clone(), VerificationLevel::Email);
    let assembly = svc_ok
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");
    let citizen = seed_citizen(&db, org).await;

    let svc = service(db.clone(), VerificationLevel::Anonymous);
    let err = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "member")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM assembly_member WHERE assembly_id = $1")
            .bind(assembly.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn add_member_rejects_unknown_role() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");
    let citizen = seed_citizen(&db, org).await;

    let err = svc
        .add_member(org, CitizenId::new(), assembly.id, citizen, "president")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn add_member_unknown_assembly_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let citizen = seed_citizen(&db, org).await;

    let err = svc
        .add_member(org, CitizenId::new(), SpaceId::new(), citizen, "member")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// list_members — keyset pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_members_paginates_by_keyset() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");

    // Add three distinct members.
    for _ in 0..3 {
        let citizen = seed_citizen(&db, org).await;
        svc.add_member(org, CitizenId::new(), assembly.id, citizen, "member")
            .await
            .expect("add member");
    }

    // First page of 2, then the remainder via the keyset cursor.
    let page1 = svc
        .list_members(assembly.id, None, Some(2))
        .await
        .expect("page1");
    assert_eq!(page1.len(), 2);
    let cursor = page1[1].id;
    let page2 = svc
        .list_members(assembly.id, Some(cursor), Some(2))
        .await
        .expect("page2");
    assert_eq!(page2.len(), 1, "remaining member after the cursor");
    assert!(page2[0].id > cursor, "strictly after the cursor");
}

// ---------------------------------------------------------------------------
// Space trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn space_gates_components_and_kind() {
    let db = connect().await;
    let space = AssembliesSpace::new(db);
    assert_eq!(space.kind(), "assemblies");
    assert!(space.allows_component("proposals"));
    assert!(space.allows_component("meetings"));
    assert!(!space.allows_component("budgets"));
    assert!(!space.allows_component("scorecard"));
}

#[tokio::test]
async fn space_ensure_open_checks_assembly_existence() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Assembleia")
        .await
        .expect("create");

    let space = AssembliesSpace::new(db);
    space
        .ensure_open(org, assembly.id)
        .await
        .expect("existing assembly is open");

    let err = space.ensure_open(org, SpaceId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn from_state_constructors_are_wired() {
    // Smoke-check the AppState-based constructors compile and build a working value path.
    let db = connect().await;
    let org = seed_org(&db).await;
    let state = dsoc_app::AppState {
        db: db.clone(),
        bus: Arc::new(dsoc_core::testing::RecordingEventBus::new()),
        authz: Arc::new(StubAuthz {
            level: VerificationLevel::Email,
        }),
        clock: Arc::new(FixedClock(now())),
    };
    let svc = AssembliesService::from_state(&state);
    let assembly = svc
        .create_assembly(org, CitizenId::new(), "Via AppState")
        .await
        .expect("create via from_state");
    let space = AssembliesSpace::from_state(&state);
    space
        .ensure_open(org, assembly.id)
        .await
        .expect("ensure_open via from_state");
    // Limits surface for clients sizing pages (read at runtime to avoid a const assertion).
    let (max, default) = (
        dsoc_assemblies::MAX_PAGE_LIMIT,
        dsoc_assemblies::DEFAULT_PAGE_LIMIT,
    );
    assert!(max >= default);
    // The role vocabulary is exposed for clients building forms.
    assert!(domain::ALLOWED_ROLES.contains(&"member"));
}
