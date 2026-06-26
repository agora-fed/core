//! Integration tests for `dsoc-surveys` against a real PostgreSQL (TESTING.md: no mocked
//! database). Each test isolates its data with a fresh `org_id`/`citizen_id` so runs never
//! collide, and uses a deterministic [`FixedClock`] (never sleeps). The crate emits no
//! cross-crate events, so the assertions are over the `survey`/`survey_question`/
//! `survey_response` state it owns and over the canonical [`dsoc_core::Error`] mapping.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_surveys::domain::{NewQuestion, NewResponse, NewSurvey};
use dsoc_surveys::service::SurveyService;
use serde_json::json;
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

fn service(db: PgPool) -> SurveyService {
    SurveyService::new(db, fixed_clock())
}

/// Seed an org (FK target for `survey.org_id`).
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

/// Seed a citizen in `org` (FK target for `survey_response.citizen_id`).
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

fn survey(title: &str) -> NewSurvey {
    NewSurvey::validate(title).expect("valid survey input")
}

fn question(prompt: &str, kind: &str, position: i32) -> NewQuestion {
    NewQuestion::validate(prompt, kind, position).expect("valid question input")
}

fn response(value: serde_json::Value) -> NewResponse {
    NewResponse::validate(value).expect("valid response input")
}

#[tokio::test]
async fn create_survey_persists_as_draft_and_is_fetchable() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let created = svc
        .create_survey(org, &survey("Satisfação com o transporte"))
        .await
        .expect("create survey");

    assert_eq!(created.org_id, org.as_uuid());
    assert_eq!(created.title, "Satisfação com o transporte");
    assert!(
        created.published_at.is_none(),
        "a freshly created survey is a draft"
    );
    assert_eq!(
        created.created_at,
        fixed_at(),
        "stamped from the injected clock"
    );

    let fetched = svc.get_survey(created.id).await.expect("get survey");
    assert_eq!(fetched, created, "the fetched row round-trips");
}

#[tokio::test]
async fn get_missing_survey_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .get_survey(Uuid::now_v7())
        .await
        .expect_err("absent survey must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn create_survey_with_unknown_org_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let ghost = OrgId::new();

    let err = svc
        .create_survey(ghost, &survey("título"))
        .await
        .expect_err("an unknown org violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn list_surveys_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let mut created = Vec::new();
    for i in 0..3 {
        let s = svc
            .create_survey(org, &survey(&format!("survey {i}")))
            .await
            .expect("create");
        created.push(s);
    }

    let (page, total) = svc.list_surveys(org, None, 2).await.expect("first page");
    assert_eq!(total, 3, "count reflects every survey of the org");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    let (next, _) = svc
        .list_surveys(org, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn list_surveys_is_scoped_to_its_org() {
    let db = pool().await;
    let svc = service(db.clone());
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;

    svc.create_survey(org_a, &survey("a")).await.expect("a");
    svc.create_survey(org_b, &survey("b")).await.expect("b");

    let (rows, total) = svc.list_surveys(org_a, None, 50).await.expect("list a");
    assert_eq!(total, 1, "org A sees only its own survey");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "a");
}

#[tokio::test]
async fn add_question_persists_typed_kind_and_position() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");

    let q = svc
        .add_question(org, s.id, &question("Qual sua nota?", "scale", 0))
        .await
        .expect("add question");

    assert_eq!(q.survey_id, s.id);
    assert_eq!(q.prompt, "Qual sua nota?");
    assert_eq!(q.kind, "scale");
    assert_eq!(q.position, 0);
    assert_eq!(q.created_at, fixed_at());
}

#[tokio::test]
async fn add_question_to_missing_survey_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .add_question(OrgId::new(), Uuid::now_v7(), &question("p", "text", 0))
        .await
        .expect_err("adding a question to a missing survey is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn questions_list_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");

    let mut created = Vec::new();
    for (i, kind) in ["text", "single_choice", "boolean"].into_iter().enumerate() {
        let q = svc
            .add_question(
                org,
                s.id,
                &question(&format!("pergunta {i}"), kind, i32::try_from(i).unwrap()),
            )
            .await
            .expect("add question");
        created.push(q);
    }

    let (page, total) = svc.list_questions(s.id, None, 2).await.expect("first page");
    assert_eq!(total, 3);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[0].kind, "text");
    assert_eq!(page[1].id, created[1].id);

    let (next, _) = svc
        .list_questions(s.id, Some(page[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
    assert_eq!(next[0].kind, "boolean");
}

#[tokio::test]
async fn publish_survey_stamps_published_at_and_double_publish_conflicts() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");
    assert!(s.published_at.is_none());

    let published = svc.publish_survey(org, s.id).await.expect("publish");
    assert_eq!(
        published.published_at,
        Some(fixed_at()),
        "publish stamps the injected clock time"
    );

    // The guarded transition (draft -> published) rejects a second publish.
    let err = svc
        .publish_survey(org, s.id)
        .await
        .expect_err("re-publishing a published survey conflicts");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn publish_missing_survey_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .publish_survey(OrgId::new(), Uuid::now_v7())
        .await
        .expect_err("publishing a missing survey is a 404");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn add_question_to_published_survey_conflicts() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");
    svc.publish_survey(org, s.id).await.expect("publish");

    let err = svc
        .add_question(org, s.id, &question("tarde demais", "text", 0))
        .await
        .expect_err("a published survey is frozen");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn submit_response_to_unpublished_survey_conflicts() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let s = svc
        .create_survey(org, &survey("Rascunho"))
        .await
        .expect("survey");

    let err = svc
        .submit_response(s.id, citizen, &response(json!({ "q1": "sim" })))
        .await
        .expect_err("a draft survey does not accept responses");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn submit_response_persists_answers_for_published_survey() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");
    svc.publish_survey(org, s.id).await.expect("publish");

    let r = svc
        .submit_response(s.id, citizen, &response(json!({ "q1": "sim", "q2": 5 })))
        .await
        .expect("submit response");

    assert_eq!(r.survey_id, s.id);
    assert_eq!(r.citizen_id, citizen.as_uuid());
    assert_eq!(r.created_at, fixed_at());
    let stored: serde_json::Value =
        serde_json::from_str(&r.answers).expect("stored answers are valid json");
    assert_eq!(stored, json!({ "q1": "sim", "q2": 5 }));
}

#[tokio::test]
async fn submit_response_is_idempotent_one_per_citizen() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");
    svc.publish_survey(org, s.id).await.expect("publish");

    let first = svc
        .submit_response(s.id, citizen, &response(json!({ "q1": "sim" })))
        .await
        .expect("first submit");

    // A second submission by the same citizen returns the existing response (the real stored
    // row id), never a duplicate — the one-per-citizen rule (ADR-0007 idempotency).
    let second = svc
        .submit_response(s.id, citizen, &response(json!({ "q1": "não" })))
        .await
        .expect("second submit is idempotent");
    assert_eq!(second.id, first.id, "the same response row is returned");

    let (rows, total) = svc.list_responses(s.id, None, 50).await.expect("list");
    assert_eq!(total, 1, "exactly one response per citizen is stored");
    assert_eq!(rows.len(), 1);
    let stored: serde_json::Value = serde_json::from_str(&rows[0].answers).expect("valid json");
    assert_eq!(
        stored,
        json!({ "q1": "sim" }),
        "the original answers are preserved"
    );
}

#[tokio::test]
async fn submit_response_with_unknown_citizen_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let ghost = CitizenId::new();

    let s = svc
        .create_survey(org, &survey("Pesquisa"))
        .await
        .expect("survey");
    svc.publish_survey(org, s.id).await.expect("publish");

    let err = svc
        .submit_response(s.id, ghost, &response(json!({ "q1": "sim" })))
        .await
        .expect_err("an unknown citizen violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn responses_list_is_keyset_paginated_and_scoped() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let s1 = svc.create_survey(org, &survey("s1")).await.expect("s1");
    let s2 = svc.create_survey(org, &survey("s2")).await.expect("s2");
    svc.publish_survey(org, s1.id).await.expect("publish s1");
    svc.publish_survey(org, s2.id).await.expect("publish s2");

    // Two distinct citizens respond to s1; one responds to s2.
    let c1 = seed_citizen(&db, org).await;
    let c2 = seed_citizen(&db, org).await;
    let c3 = seed_citizen(&db, org).await;
    let r1 = svc
        .submit_response(s1.id, c1, &response(json!({ "a": 1 })))
        .await
        .expect("r1");
    let r2 = svc
        .submit_response(s1.id, c2, &response(json!({ "a": 2 })))
        .await
        .expect("r2");
    svc.submit_response(s2.id, c3, &response(json!({ "a": 3 })))
        .await
        .expect("r3");

    let (page, total) = svc
        .list_responses(s1.id, None, 1)
        .await
        .expect("first page");
    assert_eq!(total, 2, "s1 has two responses");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, r1.id);

    let (next, _) = svc
        .list_responses(s1.id, Some(page[0].id), 10)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, r2.id);

    let (s2_rows, s2_total) = svc.list_responses(s2.id, None, 50).await.expect("list s2");
    assert_eq!(s2_total, 1, "s2 sees only its own response");
    assert_eq!(s2_rows.len(), 1);
}

#[tokio::test]
async fn cross_org_publish_and_add_question_are_forbidden() {
    let db = pool().await;
    let svc = service(db.clone());
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;

    let s = svc
        .create_survey(org_a, &survey("dona do A"))
        .await
        .expect("create in A");

    // org B must not mutate org A's survey (cross-tenant guard, ADR-0007).
    let pub_err = svc
        .publish_survey(org_b, s.id)
        .await
        .expect_err("cross-org publish must be forbidden");
    assert!(matches!(pub_err, dsoc_core::Error::Forbidden(_)));
    let q_err = svc
        .add_question(org_b, s.id, &question("intruso?", "text", 0))
        .await
        .expect_err("cross-org add_question must be forbidden");
    assert!(matches!(q_err, dsoc_core::Error::Forbidden(_)));

    // the rightful owner still can.
    svc.add_question(org_a, s.id, &question("ok?", "text", 0))
        .await
        .expect("owner adds question");
    svc.publish_survey(org_a, s.id)
        .await
        .expect("owner publishes");
}
