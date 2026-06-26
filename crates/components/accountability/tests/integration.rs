//! Integration tests for `accountability` against a real PostgreSQL (TESTING.md: no mocked
//! database).
//!
//! Each test isolates its data with a unique `org` fixture so runs never collide, uses a
//! deterministic [`FixedClock`] (never sleeps), and a [`StubAuthz`] for the authorized write
//! surface. The tests assert the full create-result / post-update flow, the derived `status`
//! invariant (progress -> pending/in_progress/achieved), the guarded terminal transition, the
//! authorization gate, validation at the boundary, and keyset pagination.

// Test code: a failed `expect`/`panic`/`unwrap` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_accountability::domain::Status;
use dsoc_accountability::queries;
use dsoc_accountability::service::AccountabilityService;
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Error, Result, VerificationLevel};
use dsoc_db::Db;
use uuid::Uuid;

/// A deterministic clock: time is injected, never read ambiently (TESTING.md).
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ))
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

async fn pool() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to test database")
}

fn service(db: Db) -> AccountabilityService {
    service_with(db, VerificationLevel::Directory)
}

fn service_with(db: Db, level: VerificationLevel) -> AccountabilityService {
    AccountabilityService::new(db, fixed_clock(), Arc::new(StubAuthz { level }))
}

/// Seed an `org` row (FK target for `accountability_result.org_id`).
async fn seed_org(db: &Db) -> OrgId {
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

#[tokio::test]
async fn create_result_starts_pending_at_zero_progress() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let result = svc
        .create_result(
            CitizenId::new(),
            org,
            "  Construir a creche  ",
            " Entrega 2026 ",
        )
        .await
        .expect("create result");

    assert_eq!(result.org_id, org.as_uuid());
    assert_eq!(result.title, "Construir a creche", "title is trimmed");
    assert_eq!(result.target, "Entrega 2026", "target is trimmed");
    assert_eq!(result.status, Status::Pending);
    assert_eq!(result.progress, 0);

    // Read back via the public getter yields the same committed row.
    let read = svc.get_result(result.id).await.expect("get result");
    assert_eq!(read.id, result.id);
    assert_eq!(read.status, Status::Pending);
}

#[tokio::test]
async fn create_result_requires_directory_verification() {
    let db = pool().await;
    let org = seed_org(&db).await;

    let weak = service_with(db.clone(), VerificationLevel::Email);
    let err = weak
        .create_result(CitizenId::new(), org, "Meta", "Alvo")
        .await
        .expect_err("under-verified caller must be forbidden");
    assert_eq!(err.code(), "forbidden");
}

#[tokio::test]
async fn create_result_rejects_empty_title() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let err = svc
        .create_result(CitizenId::new(), org, "   ", "alvo")
        .await
        .expect_err("blank title must be rejected");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn post_status_update_advances_progress_and_derives_in_progress() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let result = svc
        .create_result(CitizenId::new(), org, "Asfaltar a rua", "100% até dezembro")
        .await
        .expect("create");

    let (updated, update) = svc
        .post_status_update(CitizenId::new(), result.id, "fundação concluída", 40)
        .await
        .expect("post update");

    assert_eq!(updated.progress, 40);
    assert_eq!(updated.status, Status::InProgress);
    assert_eq!(update.result_id, result.id);
    assert_eq!(update.progress, 40);
    assert_eq!(update.note, "fundação concluída");

    // The ledger contains exactly the one update we posted.
    let updates = queries::list_status_updates(&db, result.id, None, 50)
        .await
        .expect("list updates");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].id, update.id);
}

#[tokio::test]
async fn post_status_update_to_100_marks_achieved_and_is_terminal() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let result = svc
        .create_result(CitizenId::new(), org, "Meta", "alvo")
        .await
        .expect("create");

    let (achieved, _) = svc
        .post_status_update(CitizenId::new(), result.id, "entregue", 100)
        .await
        .expect("complete");
    assert_eq!(achieved.progress, 100);
    assert_eq!(achieved.status, Status::Achieved);

    // A further update to a terminal (achieved) result is a guarded conflict.
    let err = svc
        .post_status_update(CitizenId::new(), result.id, "reabrir?", 50)
        .await
        .expect_err("achieved result must reject further updates");
    assert_eq!(err.code(), "conflict");

    // The conflicting update left no ledger row behind (only the completing one survives).
    let updates = queries::list_status_updates(&db, result.id, None, 50)
        .await
        .expect("list updates");
    assert_eq!(updates.len(), 1);
}

#[tokio::test]
async fn post_status_update_requires_directory_verification() {
    let db = pool().await;
    let org = seed_org(&db).await;
    // Create with a directory-verified author, then attempt an update as an email-only caller.
    let result = service(db.clone())
        .create_result(CitizenId::new(), org, "Meta", "alvo")
        .await
        .expect("create");

    let weak = service_with(db.clone(), VerificationLevel::Email);
    let err = weak
        .post_status_update(CitizenId::new(), result.id, "nota", 10)
        .await
        .expect_err("under-verified caller must be forbidden");
    assert_eq!(err.code(), "forbidden");

    // The forbidden attempt wrote nothing.
    let updates = queries::list_status_updates(&db, result.id, None, 50)
        .await
        .expect("list updates");
    assert!(updates.is_empty());
}

#[tokio::test]
async fn post_status_update_rejects_out_of_range_progress_and_blank_note() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let result = svc
        .create_result(CitizenId::new(), org, "Meta", "alvo")
        .await
        .expect("create");

    let err = svc
        .post_status_update(CitizenId::new(), result.id, "nota", 150)
        .await
        .expect_err("progress > 100 must be rejected");
    assert_eq!(err.code(), "invalid_input");

    let err = svc
        .post_status_update(CitizenId::new(), result.id, "   ", 10)
        .await
        .expect_err("blank note must be rejected");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn post_status_update_on_unknown_result_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .post_status_update(CitizenId::new(), Uuid::now_v7(), "nota", 10)
        .await
        .expect_err("unknown result must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn get_result_is_not_found_when_absent() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .get_result(Uuid::now_v7())
        .await
        .expect_err("absent result must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn list_status_updates_on_unknown_result_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .list_status_updates(Uuid::now_v7(), None, 50)
        .await
        .expect_err("unknown result must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn list_results_is_keyset_paginated_newest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let r1 = svc
        .create_result(CitizenId::new(), org, "Primeira", "a")
        .await
        .unwrap();
    let r2 = svc
        .create_result(CitizenId::new(), org, "Segunda", "b")
        .await
        .unwrap();

    // Both results come back; newest-first means r2 precedes r1 (UUIDv7 is time-ordered).
    let all = svc.list_results(org, None, 50).await.expect("list");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, r2.id);
    assert_eq!(all[1].id, r1.id);

    // First page of one, then a cursor past it yields the next.
    let first_page = svc.list_results(org, None, 1).await.expect("page 1");
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].id, r2.id);
    let cursor = queries::Cursor {
        at: first_page[0].created_at,
        id: first_page[0].id,
    };
    let second_page = svc
        .list_results(org, Some(cursor), 50)
        .await
        .expect("page 2");
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].id, r1.id);
}

#[tokio::test]
async fn list_status_updates_is_keyset_paginated_newest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let result = svc
        .create_result(CitizenId::new(), org, "Meta", "alvo")
        .await
        .expect("create");

    let (_, u1) = svc
        .post_status_update(CitizenId::new(), result.id, "primeiro passo", 20)
        .await
        .expect("update 1");
    let (_, u2) = svc
        .post_status_update(CitizenId::new(), result.id, "segundo passo", 60)
        .await
        .expect("update 2");

    let all = svc
        .list_status_updates(result.id, None, 50)
        .await
        .expect("list updates");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, u2.id, "newest update first");
    assert_eq!(all[1].id, u1.id);

    // The parent result reflects the latest reported progress.
    let read = svc.get_result(result.id).await.expect("get");
    assert_eq!(read.progress, 60);
    assert_eq!(read.status, Status::InProgress);
}
