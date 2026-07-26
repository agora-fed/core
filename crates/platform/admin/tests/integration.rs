//! Integration tests for `dsoc-admin` against a real PostgreSQL (docs/TESTING.md).
//!
//! Each test isolates its data with freshly generated UUIDv7 ids/slugs, injects a
//! deterministic `FixedClock` (never sleeps), and uses `dsoc_core::testing::RecordingEventBus`
//! to assert the no-cross-crate-events contract. Run with `$DATABASE_URL` pointing at an
//! IPv6-first Postgres that already has the baseline + `0150_admin_core.sql` applied.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_admin::domain::AdminRole;
use dsoc_admin::AdminService;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;
use uuid::Uuid;

/// Deterministic clock (TESTING.md: time is injected, never slept on).
#[derive(Debug)]
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

fn fixed_clock(rfc3339: &str) -> Arc<dyn Clock> {
    let at = DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc);
    Arc::new(FixedClock(at))
}

fn service_with(
    db: Db,
    clock: Arc<dyn Clock>,
    level: VerificationLevel,
) -> (AdminService, Arc<RecordingEventBus>) {
    let bus = Arc::new(RecordingEventBus::new());
    let authz = Arc::new(StubAuthz { level });
    let svc = AdminService::new(db, clock, bus.clone(), authz);
    (svc, bus)
}

async fn connect() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to test database")
}

/// Insert a baseline `org` fixture and return its id (data isolated by unique uuid/slug).
async fn seed_org(db: &Db) -> OrgId {
    let id = Uuid::now_v7();
    let slug = format!("admin-it-{id}");
    let created_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    sqlx::query!(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)",
        id,
        slug,
        "Admin IT Org",
        created_at,
    )
    .execute(db)
    .await
    .expect("seed org");
    OrgId::from_uuid(id)
}

/// Insert a baseline `citizen` fixture in `org` and return its id.
async fn seed_citizen(db: &Db, org: OrgId) -> CitizenId {
    let id = Uuid::now_v7();
    let subject = format!("oidc-{id}");
    let created_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    sqlx::query!(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, 'directory', $4)",
        id,
        org.as_uuid(),
        subject,
        created_at,
    )
    .execute(db)
    .await
    .expect("seed citizen");
    CitizenId::from_uuid(id)
}

/// Seed an `owner` binding for `citizen` in `org` — o mesmo root of trust via SQL que
/// `scripts/bootstrap-admin.sh` provê em produção. Mutações exigem isso desde o fix de
/// segurança de 2026-07-26 (só nível Directory não autoriza mais).
async fn seed_admin(db: &Db, org: OrgId, citizen: CitizenId) {
    sqlx::query!(
        "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at) \
         VALUES ($1, $2, $3, 'owner', $4)",
        Uuid::now_v7(),
        org.as_uuid(),
        citizen.as_uuid(),
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    )
    .execute(db)
    .await
    .expect("seed admin binding");
}

#[tokio::test]
async fn create_org_then_bind_role() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let actor = seed_citizen(&db, org).await;
    let member = seed_citizen(&db, org).await;
    seed_admin(&db, org, actor).await;
    let (svc, bus) = service_with(
        db,
        fixed_clock("2026-02-01T12:00:00Z"),
        VerificationLevel::Directory,
    );

    // Create the administrative org.
    let created = svc.create_org(org, actor).await.expect("create org");
    assert_eq!(created.org_id, org);
    assert!(created.is_active);

    // It is now readable.
    let fetched = svc.get_org(org).await.expect("get org");
    assert_eq!(fetched, created);

    // Bind an owner role to a citizen.
    let binding = svc
        .bind_role(org, actor, member, AdminRole::Owner)
        .await
        .expect("bind role");
    assert_eq!(binding.org_id, org);
    assert_eq!(binding.citizen_id, member);
    assert_eq!(binding.role, AdminRole::Owner);

    // The binding appears in the org's list.
    let bindings = svc
        .list_role_bindings(org, None, None)
        .await
        .expect("list bindings");
    assert!(bindings.iter().any(|b| b.id == binding.id));

    // Contract: admin emits no cross-crate events.
    assert_eq!(bus.count(), 0, "admin must emit no events");
}

#[tokio::test]
async fn duplicate_role_binding_conflicts() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let actor = seed_citizen(&db, org).await;
    let member = seed_citizen(&db, org).await;
    seed_admin(&db, org, actor).await;
    let (svc, _bus) = service_with(
        db,
        fixed_clock("2026-02-01T12:00:00Z"),
        VerificationLevel::Strong,
    );

    svc.bind_role(org, actor, member, AdminRole::Admin)
        .await
        .expect("first bind");
    let err = svc
        .bind_role(org, actor, member, AdminRole::Admin)
        .await
        .expect_err("duplicate bind must fail");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn toggle_feature_flag_is_idempotent_and_audited() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let actor = seed_citizen(&db, org).await;
    seed_admin(&db, org, actor).await;
    let key = "proposals.clustering";

    // Enable at t1.
    let (svc1, _) = service_with(
        db.clone(),
        fixed_clock("2026-03-01T00:00:00Z"),
        VerificationLevel::Directory,
    );
    let first = svc1
        .set_feature_flag(org, actor, key, true)
        .await
        .expect("enable flag");
    assert!(first.enabled);
    let created_at = first.created_at;
    assert_eq!(first.updated_at, created_at);

    // Enable again at t2 with the same value: idempotent (still one row, still enabled),
    // but audited (updated_at advances; created_at preserved).
    let (svc2, _) = service_with(
        db.clone(),
        fixed_clock("2026-03-02T00:00:00Z"),
        VerificationLevel::Directory,
    );
    let second = svc2
        .set_feature_flag(org, actor, key, true)
        .await
        .expect("re-enable flag");
    assert_eq!(second.id, first.id, "idempotent: same row, no duplicate");
    assert!(second.enabled);
    assert_eq!(second.created_at, created_at, "created_at preserved");
    assert!(
        second.updated_at > created_at,
        "updated_at advanced (audited)"
    );

    // Exactly one flag exists for this (org, key).
    let flags = svc2
        .list_feature_flags(org, None, None)
        .await
        .expect("list flags");
    assert_eq!(flags.iter().filter(|f| f.key == key).count(), 1);

    // Flip to disabled at t3.
    let (svc3, _) = service_with(
        db,
        fixed_clock("2026-03-03T00:00:00Z"),
        VerificationLevel::Directory,
    );
    let third = svc3
        .set_feature_flag(org, actor, key, false)
        .await
        .expect("disable flag");
    assert!(!third.enabled);
    assert_eq!(third.id, first.id);
    assert_eq!(third.created_at, created_at);
}

#[tokio::test]
async fn unauthorized_mutation_is_forbidden() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let actor = seed_citizen(&db, org).await;
    // Caller resolves only to Email level — below the directory bar for mutations.
    let (svc, _bus) = service_with(
        db,
        fixed_clock("2026-02-01T12:00:00Z"),
        VerificationLevel::Email,
    );

    let err = svc
        .create_org(org, actor)
        .await
        .expect_err("create must be forbidden");
    assert_eq!(err.code(), "forbidden");

    // Nothing was persisted.
    let missing = svc.get_org(org).await.expect_err("org must not exist");
    assert_eq!(missing.code(), "not_found");
}

/// R0.3 (#40): `permissions_for` agrega papéis bindados + o papel Base implícito,
/// e resolve chaves `modulo.acao`. Semeia papéis pro org de teste (a 0600 só semeou
/// orgs que já existiam quando rodou), binda um Moderador e confere.
#[tokio::test]
async fn permissions_for_aggregates_roles_and_base() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let moderador = seed_citizen(&db, org).await;
    let anon = seed_citizen(&db, org).await; // sem binding — só Base implícito.

    // Base (pos 0, vazio) + Moderador (content.moderate) pro org de teste.
    let base_id = Uuid::now_v7();
    let mod_id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO user_role (id, org_id, name, position, permissions) \
         VALUES ($1, $2, 'Base', 0, ARRAY[]::text[])",
        base_id,
        org.as_uuid(),
    )
    .execute(&db)
    .await
    .expect("seed base role");
    sqlx::query!(
        "INSERT INTO user_role (id, org_id, name, position, permissions) \
         VALUES ($1, $2, 'Moderador', 10, ARRAY['content.moderate','reports.manage'])",
        mod_id,
        org.as_uuid(),
    )
    .execute(&db)
    .await
    .expect("seed moderador role");
    sqlx::query!(
        "INSERT INTO citizen_role_binding (id, org_id, citizen_id, role_id, created_at) \
         VALUES ($1, $2, $3, $4, now())",
        Uuid::now_v7(),
        org.as_uuid(),
        moderador.as_uuid(),
        mod_id,
    )
    .execute(&db)
    .await
    .expect("bind moderador");

    let (svc, _bus) = service_with(
        db,
        fixed_clock("2026-02-01T12:00:00Z"),
        VerificationLevel::Email,
    );

    // O moderador resolve content.moderate/reports.manage, mas não roles.manage.
    let p = svc
        .permissions_for(org, moderador)
        .await
        .expect("perms mod");
    assert!(p.can("content.moderate"));
    assert!(p.can("reports.manage"));
    assert!(!p.can("roles.manage"));
    assert!(!p.is_administrator());

    // Sem binding: só o Base (vazio) — não pode nada.
    let p2 = svc.permissions_for(org, anon).await.expect("perms anon");
    assert!(!p2.can("content.moderate"));
    assert!(p2.is_empty());
}

/// Regressão do fix de 2026-07-26: um cidadão nível Directory SEM binding admin
/// não pode mais mutar (antes, `bind_role` deixava ele se autopromover a owner).
#[tokio::test]
async fn directory_without_admin_binding_is_forbidden() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let actor = seed_citizen(&db, org).await; // Directory, mas sem binding.
    let member = seed_citizen(&db, org).await;
    let (svc, _bus) = service_with(
        db,
        fixed_clock("2026-02-01T12:00:00Z"),
        VerificationLevel::Directory,
    );

    // Autopromoção bloqueada.
    let err = svc
        .bind_role(org, actor, member, AdminRole::Owner)
        .await
        .expect_err("sem binding admin, bind_role deve ser forbidden");
    assert_eq!(err.code(), "forbidden");

    let err2 = svc
        .set_feature_flag(org, actor, "proposals.clustering", true)
        .await
        .expect_err("sem binding admin, set_feature_flag deve ser forbidden");
    assert_eq!(err2.code(), "forbidden");
}
