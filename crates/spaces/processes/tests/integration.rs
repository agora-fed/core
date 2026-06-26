//! Integration tests for `dsoc-processes` against a real PostgreSQL (docs/TESTING.md: no mocked
//! DB). Each test isolates its data behind a freshly generated `OrgId`, injects a deterministic
//! `FixedClock` (never sleeps), and authorizes through a `StubAuthz` resolving a fixed verification
//! level. The crate emits no cross-crate events, so there is no outbox to assert on.
//!
//! Requires `DATABASE_URL` and the baseline + `0210_processes_core.sql` applied.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;
use dsoc_processes::service::{ProcessDraft, ProcessUpdate};
use dsoc_processes::{ProcessService, ProcessStatus, ProcessesSpace};

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

/// Build a service at the fixed clock instant and caller verification level.
fn service(db: Db, level: VerificationLevel) -> ProcessService {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now()));
    let authz = Arc::new(StubAuthz { level });
    ProcessService::new(db, clock, authz)
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

fn draft(slug: &str) -> ProcessDraft {
    ProcessDraft {
        slug: slug.to_owned(),
        title: "Orçamento Participativo".to_owned(),
        status: None,
        starts_at: None,
        ends_at: None,
    }
}

// ---------------------------------------------------------------------------
// create / get
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_returns_real_row_and_defaults_to_draft() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let created = svc
        .create_process(org, CitizenId::new(), draft("orcamento-2026"))
        .await
        .expect("create");

    assert_eq!(created.slug, "orcamento-2026");
    assert_eq!(created.status, ProcessStatus::Draft);
    assert_eq!(created.created_at, now());

    // The id is the REAL stored row, fetchable back.
    let fetched = svc.get_process(org, created.id).await.expect("get");
    assert_eq!(fetched, created);
}

#[tokio::test]
async fn create_is_forbidden_below_directory_level() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);

    let err = svc
        .create_process(org, CitizenId::new(), draft("plano-diretor"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");
}

#[tokio::test]
async fn create_rejects_invalid_slug() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc
        .create_process(org, CitizenId::new(), draft("INVALID SLUG"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn create_rejects_inverted_window() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let bad = ProcessDraft {
        starts_at: Some(now()),
        ends_at: Some(now() - chrono::Duration::hours(1)),
        ..draft("janela")
    };
    let err = svc
        .create_process(org, CitizenId::new(), bad)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn create_conflicts_on_duplicate_slug() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    svc.create_process(org, CitizenId::new(), draft("dup"))
        .await
        .expect("first");
    let err = svc
        .create_process(org, CitizenId::new(), draft("dup"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn get_missing_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc.get_process(org, Uuid::now_v7()).await.unwrap_err();
    assert_eq!(err.code(), "not_found");
}

// ---------------------------------------------------------------------------
// list (keyset pagination)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_paginates_by_keyset() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    for i in 0..3 {
        svc.create_process(org, CitizenId::new(), draft(&format!("proc-{i}")))
            .await
            .expect("seed");
    }

    let page1 = svc.list_processes(org, None, Some(2)).await.expect("page1");
    assert_eq!(page1.len(), 2);
    let cursor = page1.last().unwrap().id;
    let page2 = svc
        .list_processes(org, Some(cursor), Some(2))
        .await
        .expect("page2");
    assert_eq!(page2.len(), 1);
    // Ascending, strictly after the cursor — no overlap.
    assert!(page2[0].id > cursor);
}

// ---------------------------------------------------------------------------
// update / delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_changes_title_and_status() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let created = svc
        .create_process(org, CitizenId::new(), draft("editavel"))
        .await
        .expect("create");

    let updated = svc
        .update_process(
            org,
            CitizenId::new(),
            created.id,
            ProcessUpdate {
                title: Some("Novo Título".to_owned()),
                status: Some(ProcessStatus::Active),
            },
        )
        .await
        .expect("update");

    assert_eq!(updated.title, "Novo Título");
    assert_eq!(updated.status, ProcessStatus::Active);
    // Slug is immutable through update.
    assert_eq!(updated.slug, "editavel");
}

#[tokio::test]
async fn update_missing_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc
        .update_process(
            org,
            CitizenId::new(),
            Uuid::now_v7(),
            ProcessUpdate {
                title: Some("x".to_owned()),
                status: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn delete_removes_process_and_cascades_phases() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let created = svc
        .create_process(org, CitizenId::new(), draft("removivel"))
        .await
        .expect("create");
    svc.add_phase(org, CitizenId::new(), created.id, "Fase 1", 0)
        .await
        .expect("phase");

    svc.delete_process(org, CitizenId::new(), created.id)
        .await
        .expect("delete");

    assert_eq!(
        svc.get_process(org, created.id).await.unwrap_err().code(),
        "not_found"
    );
    // The cascade removed the phase too.
    let remaining = svc.list_phases(created.id, None, None).await.expect("list");
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn delete_missing_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc
        .delete_process(org, CitizenId::new(), Uuid::now_v7())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

// ---------------------------------------------------------------------------
// phases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_phase_into_missing_process_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc
        .add_phase(org, CitizenId::new(), Uuid::now_v7(), "Fase", 0)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn add_phase_conflicts_on_duplicate_position() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let p = svc
        .create_process(org, CitizenId::new(), draft("fases"))
        .await
        .expect("create");

    svc.add_phase(org, CitizenId::new(), p.id, "A", 0)
        .await
        .expect("first");
    let err = svc
        .add_phase(org, CitizenId::new(), p.id, "B", 0)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn list_phases_orders_by_position() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let p = svc
        .create_process(org, CitizenId::new(), draft("ordenadas"))
        .await
        .expect("create");

    for (name, pos) in [("C", 2), ("A", 0), ("B", 1)] {
        svc.add_phase(org, CitizenId::new(), p.id, name, pos)
            .await
            .expect("add");
    }

    let phases = svc.list_phases(p.id, None, None).await.expect("list");
    let positions: Vec<i32> = phases.iter().map(|ph| ph.position).collect();
    assert_eq!(positions, vec![0, 1, 2]);
    assert!(phases.iter().all(|ph| !ph.active));
}

// ---------------------------------------------------------------------------
// advance phase (the guarded state transition)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn advance_walks_phases_in_order_then_conflicts_at_end() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let p = svc
        .create_process(org, CitizenId::new(), draft("caminhada"))
        .await
        .expect("create");
    for (name, pos) in [("Propostas", 0), ("Debate", 1), ("Votação", 2)] {
        svc.add_phase(org, CitizenId::new(), p.id, name, pos)
            .await
            .expect("add");
    }

    // First advance activates the first phase (none active yet).
    let first = svc
        .advance_phase(org, CitizenId::new(), p.id)
        .await
        .expect("a0");
    assert_eq!(first.position, 0);
    assert!(first.active);

    let second = svc
        .advance_phase(org, CitizenId::new(), p.id)
        .await
        .expect("a1");
    assert_eq!(second.position, 1);

    let third = svc
        .advance_phase(org, CitizenId::new(), p.id)
        .await
        .expect("a2");
    assert_eq!(third.position, 2);

    // Exactly one phase is active at the end.
    let phases = svc.list_phases(p.id, None, None).await.expect("list");
    assert_eq!(phases.iter().filter(|ph| ph.active).count(), 1);
    assert!(phases.iter().find(|ph| ph.position == 2).unwrap().active);

    // No further phase to advance to.
    let err = svc
        .advance_phase(org, CitizenId::new(), p.id)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn advance_on_closed_process_conflicts() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let p = svc
        .create_process(org, CitizenId::new(), draft("fechado"))
        .await
        .expect("create");
    svc.add_phase(org, CitizenId::new(), p.id, "Única", 0)
        .await
        .expect("phase");
    svc.update_process(
        org,
        CitizenId::new(),
        p.id,
        ProcessUpdate {
            title: None,
            status: Some(ProcessStatus::Closed),
        },
    )
    .await
    .expect("close");

    let err = svc
        .advance_phase(org, CitizenId::new(), p.id)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn advance_on_missing_process_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);

    let err = svc
        .advance_phase(org, CitizenId::new(), Uuid::now_v7())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn advance_is_forbidden_below_directory_level() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Email);

    let err = svc
        .advance_phase(org, CitizenId::new(), Uuid::now_v7())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "forbidden");
}

// ---------------------------------------------------------------------------
// Space trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn space_kind_and_component_gate() {
    let db = connect().await;
    let space = ProcessesSpace::new(db);
    assert_eq!(space.kind(), "processes");
    assert!(space.allows_component("proposals"));
    assert!(space.allows_component("budgets"));
    assert!(!space.allows_component("scorecard"));
}

#[tokio::test]
async fn ensure_open_requires_active_status() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone(), VerificationLevel::Directory);
    let space = ProcessesSpace::new(db.clone());

    let drafted = svc
        .create_process(org, CitizenId::new(), draft("aberto"))
        .await
        .expect("create");

    // A draft process is not open for participation.
    let err = space
        .ensure_open(org, SpaceId::from_uuid(drafted.id))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "conflict");

    // Activate it, then it is open.
    svc.update_process(
        org,
        CitizenId::new(),
        drafted.id,
        ProcessUpdate {
            title: None,
            status: Some(ProcessStatus::Active),
        },
    )
    .await
    .expect("activate");
    space
        .ensure_open(org, SpaceId::from_uuid(drafted.id))
        .await
        .expect("now open");
}

#[tokio::test]
async fn ensure_open_missing_process_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let space = ProcessesSpace::new(db);

    let err = space
        .ensure_open(org, SpaceId::from_uuid(Uuid::now_v7()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}
