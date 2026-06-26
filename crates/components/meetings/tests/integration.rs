//! Integration tests for `dsoc-meetings` against a real PostgreSQL (TESTING.md: no mocked
//! database). Each test isolates its data with a fresh `org_id`/`citizen_id` so runs never
//! collide, and uses a deterministic [`FixedClock`] (never sleeps). The crate emits no
//! cross-crate events, so the assertions are over the `meeting`/`meeting_attendee` state it owns
//! and over the canonical [`dsoc_core::Error`] mapping.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_meetings::domain::{NewMeeting, ValidMinutes};
use dsoc_meetings::service::MeetingService;
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

fn starts_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 1, 19, 0, 0).unwrap()
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

fn service(db: PgPool) -> MeetingService {
    MeetingService::new(db, fixed_clock())
}

/// Seed an org (FK target for `meeting.org_id`).
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

/// Seed a citizen in `org` (FK target for `meeting_attendee.citizen_id`).
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

fn meeting(title: &str, location: &str) -> NewMeeting {
    NewMeeting::validate(title, location, starts_at()).expect("valid meeting input")
}

#[tokio::test]
async fn schedule_meeting_persists_and_is_fetchable() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let created = svc
        .schedule(org, &meeting("Assembleia do bairro", "Praça Central"))
        .await
        .expect("schedule meeting");

    assert_eq!(created.org_id, org.as_uuid());
    assert_eq!(created.title, "Assembleia do bairro");
    assert_eq!(created.location, "Praça Central");
    assert_eq!(created.starts_at, starts_at());
    assert!(created.minutes.is_none(), "minutes absent until set");
    assert_eq!(
        created.created_at,
        fixed_at(),
        "stamped from the injected clock"
    );

    let fetched = svc.get_meeting(created.id).await.expect("get meeting");
    assert_eq!(fetched, created, "the fetched row round-trips");
}

#[tokio::test]
async fn get_missing_meeting_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .get_meeting(Uuid::now_v7())
        .await
        .expect_err("absent meeting must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn schedule_with_unknown_org_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    // Never-seeded org: the FK to org(id) is violated and maps to a clean Validation error.
    let ghost = OrgId::new();

    let err = svc
        .schedule(ghost, &meeting("título", "local"))
        .await
        .expect_err("an unknown org violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn list_meetings_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    // Schedule three meetings; ids are UUIDv7 (time-ordered), so creation order == sort order.
    let mut created = Vec::new();
    for i in 0..3 {
        let m = svc
            .schedule(org, &meeting(&format!("reunião {i}"), "local"))
            .await
            .expect("schedule");
        created.push(m);
    }

    let (page, total) = svc.list_meetings(org, None, 2).await.expect("first page");
    assert_eq!(total, 3, "count reflects every meeting of the org");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    // Page past the second row via the keyset cursor.
    let (next, _) = svc
        .list_meetings(org, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn list_meetings_is_scoped_to_its_org() {
    let db = pool().await;
    let svc = service(db.clone());
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;

    svc.schedule(org_a, &meeting("a", "local"))
        .await
        .expect("a");
    svc.schedule(org_b, &meeting("b", "local"))
        .await
        .expect("b");

    let (rows, total) = svc.list_meetings(org_a, None, 50).await.expect("list a");
    assert_eq!(total, 1, "org A sees only its own meeting");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "a");
}

#[tokio::test]
async fn set_minutes_replaces_and_persists() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let m = svc
        .schedule(org, &meeting("Assembleia", "local"))
        .await
        .expect("schedule");
    assert!(m.minutes.is_none());

    let updated = svc
        .set_minutes(
            m.id,
            &ValidMinutes::validate("Decidimos pavimentar a rua").expect("valid minutes"),
        )
        .await
        .expect("set minutes");
    assert_eq!(updated.id, m.id);
    assert_eq!(
        updated.minutes.as_deref(),
        Some("Decidimos pavimentar a rua")
    );

    // The replacement is durable on re-read.
    let again = svc.get_meeting(m.id).await.expect("re-read");
    assert_eq!(again.minutes.as_deref(), Some("Decidimos pavimentar a rua"));

    // And it can be replaced again.
    let replaced = svc
        .set_minutes(
            m.id,
            &ValidMinutes::validate("Correção: adiamos a decisão").expect("valid minutes"),
        )
        .await
        .expect("replace minutes");
    assert_eq!(
        replaced.minutes.as_deref(),
        Some("Correção: adiamos a decisão")
    );
}

#[tokio::test]
async fn set_minutes_on_missing_meeting_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .set_minutes(
            Uuid::now_v7(),
            &ValidMinutes::validate("ata").expect("valid minutes"),
        )
        .await
        .expect_err("setting minutes on a missing meeting is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn attend_persists_with_citizen_and_is_idempotent() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let m = svc
        .schedule(org, &meeting("Assembleia", "local"))
        .await
        .expect("schedule");

    let first = svc.attend(m.id, citizen).await.expect("attend");
    assert_eq!(first.meeting_id, m.id);
    assert_eq!(first.citizen_id, citizen.as_uuid());
    assert_eq!(first.created_at, fixed_at());

    // A repeat RSVP is idempotent: it returns the same stored row, not a duplicate.
    let second = svc.attend(m.id, citizen).await.expect("attend again");
    assert_eq!(
        second.id, first.id,
        "idempotent insert returns the real row id"
    );

    let (rows, total) = svc.list_attendees(m.id, None, 50).await.expect("list");
    assert_eq!(total, 1, "a repeated RSVP records a single attendance");
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn attend_missing_meeting_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let err = svc
        .attend(Uuid::now_v7(), citizen)
        .await
        .expect_err("attending a missing meeting is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn attend_unknown_citizen_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let m = svc
        .schedule(org, &meeting("Assembleia", "local"))
        .await
        .expect("schedule");
    // Never-seeded citizen: the FK to citizen(id) is violated and maps to Validation.
    let ghost = CitizenId::new();

    let err = svc
        .attend(m.id, ghost)
        .await
        .expect_err("an unknown citizen violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn attendees_list_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let m = svc
        .schedule(org, &meeting("Assembleia", "local"))
        .await
        .expect("schedule");

    // Three distinct citizens RSVP.
    let mut created = Vec::new();
    for _ in 0..3 {
        let citizen = seed_citizen(&db, org).await;
        let a = svc.attend(m.id, citizen).await.expect("attend");
        created.push(a);
    }

    let (page, total) = svc.list_attendees(m.id, None, 2).await.expect("first page");
    assert_eq!(total, 3);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    let (next, _) = svc
        .list_attendees(m.id, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn attendees_are_scoped_to_their_meeting() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let m1 = svc
        .schedule(org, &meeting("m1", "local"))
        .await
        .expect("m1");
    let m2 = svc
        .schedule(org, &meeting("m2", "local"))
        .await
        .expect("m2");

    svc.attend(m1.id, citizen).await.expect("attend m1");

    let (rows, total) = svc.list_attendees(m2.id, None, 50).await.expect("list m2");
    assert_eq!(total, 0, "a meeting sees only its own attendees");
    assert!(rows.is_empty());
}
