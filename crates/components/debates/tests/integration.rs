//! Integration tests for `dsoc-debates` against a real PostgreSQL (TESTING.md: no mocked
//! database). Each test isolates its data with a fresh `org_id`/`citizen_id` so runs never
//! collide, and uses a deterministic [`FixedClock`] (never sleeps). The crate emits no
//! cross-crate events, so the assertions are over the `debate`/`debate_contribution` state it
//! owns and over the canonical [`dsoc_core::Error`] mapping.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_debates::domain::{NewContribution, NewDebate};
use dsoc_debates::service::DebateService;
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

fn service(db: PgPool) -> DebateService {
    DebateService::new(db, fixed_clock())
}

/// Seed an org (FK target for `debate.org_id`).
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

/// Seed a citizen in `org` (FK target for `debate_contribution.author_id`).
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

fn debate(title: &str, framing: &str) -> NewDebate {
    NewDebate::validate(title, framing, None).expect("valid debate input")
}

fn contribution(stance: &str, body: &str) -> NewContribution {
    NewContribution::validate(stance, body).expect("valid contribution input")
}

#[tokio::test]
async fn create_debate_persists_and_is_fetchable() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let created = svc
        .create_debate(
            org,
            &debate("Tarifa zero?", "Debate sobre transporte público"),
        )
        .await
        .expect("create debate");

    assert_eq!(created.org_id, org.as_uuid());
    assert_eq!(created.title, "Tarifa zero?");
    assert_eq!(created.framing, "Debate sobre transporte público");
    assert_eq!(
        created.created_at,
        fixed_at(),
        "stamped from the injected clock"
    );

    let fetched = svc.get_debate(created.id).await.expect("get debate");
    assert_eq!(fetched, created, "the fetched row round-trips");
}

#[tokio::test]
async fn get_missing_debate_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .get_debate(Uuid::now_v7())
        .await
        .expect_err("absent debate must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn create_debate_with_unknown_org_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    // Never-seeded org: the FK to org(id) is violated and maps to a clean Validation error.
    let ghost = OrgId::new();

    let err = svc
        .create_debate(ghost, &debate("título", "enquadramento"))
        .await
        .expect_err("an unknown org violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn list_debates_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    // Create three debates; ids are UUIDv7 (time-ordered), so creation order == sort order.
    let mut created = Vec::new();
    for i in 0..3 {
        let d = svc
            .create_debate(org, &debate(&format!("debate {i}"), "framing"))
            .await
            .expect("create");
        created.push(d);
    }

    let (page, total) = svc.list_debates(org, None, 2).await.expect("first page");
    assert_eq!(total, 3, "count reflects every debate of the org");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    // Page past the second row via the keyset cursor.
    let (next, _) = svc
        .list_debates(org, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn list_debates_is_scoped_to_its_org() {
    let db = pool().await;
    let svc = service(db.clone());
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;

    svc.create_debate(org_a, &debate("a", "framing"))
        .await
        .expect("a");
    svc.create_debate(org_b, &debate("b", "framing"))
        .await
        .expect("b");

    let (rows, total) = svc.list_debates(org_a, None, 50).await.expect("list a");
    assert_eq!(total, 1, "org A sees only its own debate");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "a");
}

#[tokio::test]
async fn contribute_persists_with_stance_and_author() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;

    let d = svc
        .create_debate(org, &debate("Tarifa zero?", "framing"))
        .await
        .expect("debate");

    let c = svc
        .contribute(d.id, author, &contribution("pro", "sou a favor"))
        .await
        .expect("contribute");

    assert_eq!(c.debate_id, d.id);
    assert_eq!(c.author_id, author.as_uuid());
    assert_eq!(c.stance, "pro");
    assert_eq!(c.body, "sou a favor");
    assert_eq!(c.created_at, fixed_at());
}

#[tokio::test]
async fn contribute_to_missing_debate_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;

    let err = svc
        .contribute(Uuid::now_v7(), author, &contribution("con", "discordo"))
        .await
        .expect_err("contributing to a missing debate is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn contributions_list_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;

    let d = svc
        .create_debate(org, &debate("debate", "framing"))
        .await
        .expect("debate");

    // Three contributions across the full stance set.
    let mut created = Vec::new();
    for (stance, body) in [("pro", "a favor"), ("con", "contra"), ("neutral", "talvez")] {
        let c = svc
            .contribute(d.id, author, &contribution(stance, body))
            .await
            .expect("contribute");
        created.push(c);
    }

    let (page, total) = svc
        .list_contributions(d.id, None, 2)
        .await
        .expect("first page");
    assert_eq!(total, 3);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[0].stance, "pro");
    assert_eq!(page[1].id, created[1].id);

    let (next, _) = svc
        .list_contributions(d.id, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
    assert_eq!(next[0].stance, "neutral");
}

#[tokio::test]
async fn contributions_are_scoped_to_their_debate() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;

    let d1 = svc
        .create_debate(org, &debate("d1", "framing"))
        .await
        .expect("d1");
    let d2 = svc
        .create_debate(org, &debate("d2", "framing"))
        .await
        .expect("d2");

    svc.contribute(d1.id, author, &contribution("pro", "no debate 1"))
        .await
        .expect("contribute d1");

    let (rows, total) = svc
        .list_contributions(d2.id, None, 50)
        .await
        .expect("list d2");
    assert_eq!(total, 0, "a debate sees only its own contributions");
    assert!(rows.is_empty());
}
