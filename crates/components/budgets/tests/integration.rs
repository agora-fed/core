//! Integration tests for `dsoc-budgets` against a real PostgreSQL (TESTING.md: no mocked
//! database). Each test isolates its data with a fresh `org_id`/`citizen_id` so runs never
//! collide, and uses a deterministic [`FixedClock`] (never sleeps). The crate emits no
//! cross-crate events, so the assertions are over the `budget`/`budget_project`/`budget_order`
//! /`budget_order_item` state it owns and over the canonical [`dsoc_core::Error`] mapping —
//! in particular the **allocation rule** (an order must not exceed the budget ceiling).

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_budgets::domain::{NewBudget, NewProject};
use dsoc_budgets::service::BudgetService;
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, OrgId};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// A deterministic clock: time is injected, never read ambiently (TESTING.md).
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn fixed_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap()
}

fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(fixed_at()))
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

fn service(db: PgPool) -> BudgetService {
    BudgetService::new(db, fixed_clock())
}

/// Seed an org (FK target for `budget.org_id`).
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

/// Seed a citizen in `org` (FK target for `budget_order.citizen_id`).
async fn seed_citizen(db: &PgPool, org: OrgId) -> CitizenId {
    let citizen = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, 'email', now())",
    )
    .bind(citizen.as_uuid())
    .bind(org.as_uuid())
    .bind(format!("sub-{}", citizen.as_uuid()))
    .execute(db)
    .await
    .expect("seed citizen");
    citizen
}

fn budget(title: &str, ceiling_cents: i64) -> NewBudget {
    NewBudget::validate(title, ceiling_cents).expect("valid budget input")
}

fn project(title: &str, cost_cents: i64) -> NewProject {
    NewProject::validate(title, cost_cents).expect("valid project input")
}

#[tokio::test]
async fn create_budget_persists_and_is_fetchable() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let created = svc
        .create_budget(org, &budget("Orçamento 2026", 1_000_000))
        .await
        .expect("create budget");

    assert_eq!(created.org_id, org.as_uuid());
    assert_eq!(created.title, "Orçamento 2026");
    assert_eq!(created.ceiling_cents, 1_000_000);
    assert_eq!(
        created.created_at,
        fixed_at(),
        "stamped from the injected clock"
    );

    let fetched = svc.get_budget(created.id).await.expect("get budget");
    assert_eq!(fetched, created, "the fetched row round-trips");
}

#[tokio::test]
async fn get_missing_budget_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .get_budget(Uuid::now_v7())
        .await
        .expect_err("absent budget must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn create_budget_with_unknown_org_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let ghost = OrgId::new();

    let err = svc
        .create_budget(ghost, &budget("título", 500))
        .await
        .expect_err("an unknown org violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn list_budgets_is_keyset_paginated_and_scoped_to_org() {
    let db = pool().await;
    let svc = service(db.clone());
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;

    let mut created = Vec::new();
    for i in 0..3 {
        let b = svc
            .create_budget(org_a, &budget(&format!("b{i}"), 1_000))
            .await
            .expect("create");
        created.push(b);
    }
    svc.create_budget(org_b, &budget("other", 1_000))
        .await
        .expect("b in org_b");

    let (page, total) = svc.list_budgets(org_a, None, 2).await.expect("first page");
    assert_eq!(total, 3, "count is scoped to org_a");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    let (next, _) = svc
        .list_budgets(org_a, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn add_project_persists_and_lists_under_its_budget() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000_000))
        .await
        .expect("budget");

    let p = svc
        .add_project(b.id, &project("Praça nova", 50_000))
        .await
        .expect("add project");
    assert_eq!(p.budget_id, b.id);
    assert_eq!(p.cost_cents, 50_000);
    assert_eq!(p.created_at, fixed_at());

    let (rows, total) = svc.list_projects(b.id, None, 50).await.expect("list");
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, p.id);
}

#[tokio::test]
async fn add_project_to_missing_budget_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .add_project(Uuid::now_v7(), &project("x", 10))
        .await
        .expect_err("missing budget is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn place_order_within_ceiling_persists_items_and_total() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");
    let p1 = svc
        .add_project(b.id, &project("Praça", 400))
        .await
        .expect("p1");
    let p2 = svc
        .add_project(b.id, &project("Ciclovia", 500))
        .await
        .expect("p2");

    let placed = svc
        .place_order(b.id, citizen, &[p1.id, p2.id])
        .await
        .expect("order within ceiling");

    assert_eq!(placed.order.budget_id, b.id);
    assert_eq!(placed.order.citizen_id, citizen.as_uuid());
    assert_eq!(placed.order.created_at, fixed_at());
    assert_eq!(
        placed.allocated_cents, 900,
        "400 + 500, under the 1000 ceiling"
    );
    assert_eq!(placed.project_ids.len(), 2);

    // The order items round-trip from storage.
    let mut expected = vec![p1.id, p2.id];
    expected.sort();
    let stored = svc
        .order_project_ids(placed.order.id)
        .await
        .expect("read items");
    assert_eq!(stored, expected);

    let (orders, total) = svc.list_orders(b.id, None, 50).await.expect("list orders");
    assert_eq!(total, 1);
    assert_eq!(orders[0].id, placed.order.id);
}

#[tokio::test]
async fn place_order_at_exactly_the_ceiling_is_allowed() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");
    let p = svc
        .add_project(b.id, &project("Tudo", 1_000))
        .await
        .expect("p");

    let placed = svc
        .place_order(b.id, citizen, &[p.id])
        .await
        .expect("an allocation exactly at the ceiling is admissible");
    assert_eq!(placed.allocated_cents, 1_000);
}

#[tokio::test]
async fn place_order_exceeding_ceiling_is_conflict() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");
    let p1 = svc
        .add_project(b.id, &project("Praça", 600))
        .await
        .expect("p1");
    let p2 = svc
        .add_project(b.id, &project("Ciclovia", 600))
        .await
        .expect("p2");

    let err = svc
        .place_order(b.id, citizen, &[p1.id, p2.id])
        .await
        .expect_err("1200 exceeds the 1000 ceiling");
    assert_eq!(err.code(), "conflict");

    // The failed order must not have persisted anything (transactional rollback).
    let (orders, total) = svc.list_orders(b.id, None, 50).await.expect("list");
    assert_eq!(total, 0, "an over-ceiling order leaves no row behind");
    assert!(orders.is_empty());
}

#[tokio::test]
async fn place_order_with_no_projects_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");

    let err = svc
        .place_order(b.id, citizen, &[])
        .await
        .expect_err("an empty order is invalid");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn place_order_with_foreign_project_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let b1 = svc
        .create_budget(org, &budget("B1", 1_000))
        .await
        .expect("b1");
    let b2 = svc
        .create_budget(org, &budget("B2", 1_000))
        .await
        .expect("b2");
    // A project that belongs to b2, used in an order against b1.
    let foreign = svc
        .add_project(b2.id, &project("alheio", 100))
        .await
        .expect("foreign project");

    let err = svc
        .place_order(b1.id, citizen, &[foreign.id])
        .await
        .expect_err("a project of another budget is not allocatable here");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn place_order_to_missing_budget_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let err = svc
        .place_order(Uuid::now_v7(), citizen, &[Uuid::now_v7()])
        .await
        .expect_err("ordering against a missing budget is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn second_order_by_same_citizen_is_conflict() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");
    let p = svc
        .add_project(b.id, &project("Praça", 100))
        .await
        .expect("p");

    svc.place_order(b.id, citizen, &[p.id])
        .await
        .expect("first order succeeds");

    let err = svc
        .place_order(b.id, citizen, &[p.id])
        .await
        .expect_err("one order per citizen per budget");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn duplicate_project_ids_are_deduplicated() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let b = svc
        .create_budget(org, &budget("Orçamento", 1_000))
        .await
        .expect("budget");
    let p = svc
        .add_project(b.id, &project("Praça", 400))
        .await
        .expect("p");

    // The same project listed twice must be counted once (400, not 800).
    let placed = svc
        .place_order(b.id, citizen, &[p.id, p.id])
        .await
        .expect("dedup keeps the allocation at a single project's cost");
    assert_eq!(placed.allocated_cents, 400);
    assert_eq!(placed.project_ids, vec![p.id]);
}
