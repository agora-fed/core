//! Integration tests for `dsoc-consultations` against a real PostgreSQL (TESTING.md: no mocked DB).
//! Each test isolates its data behind a unique `OrgId`, uses a deterministic `FixedClock` (never
//! sleeps), and exercises the service, the `Space` impl, and the Axum handlers end to end.
//!
//! Requires `DATABASE_URL` and the `0240_consultations_*` migration applied (the harness does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Duration, Utc};

use dsoc_app::{AppState, CallerId};
use dsoc_consultations::http::ListParams;
use dsoc_consultations::{ConsultationService, ConsultationSpace, CreateConsultationRequest};
use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::traits::Space;
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;

/// Deterministic clock — timing is reproducible without sleeping.
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// A test double for [`Authorization`] that grants a fixed level to everyone (the trait, not the
/// concrete `auth` crate, is what this crate depends on). `require` mirrors the real semantics.
#[derive(Debug, Clone, Copy)]
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
            Err(Error::Forbidden("below required level".to_owned()))
        }
    }
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn opens() -> DateTime<Utc> {
    now() + Duration::days(1)
}

fn closes() -> DateTime<Utc> {
    now() + Duration::days(8)
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

fn service(db: Db) -> ConsultationService {
    ConsultationService::new(db, Arc::new(FixedClock(now())))
}

fn app_state(db: Db, level: VerificationLevel) -> AppState {
    AppState {
        db,
        bus: Arc::new(RecordingEventBus::new()) as Arc<dyn EventBus>,
        authz: Arc::new(StubAuthz { level }) as Arc<dyn Authorization>,
        clock: Arc::new(FixedClock(now())) as Arc<dyn Clock>,
        storage: None,
    }
}

fn caller(org: OrgId) -> CallerId {
    CallerId {
        citizen: CitizenId::new(),
        org,
    }
}

fn prompts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("Pergunta {i}?")).collect()
}

#[tokio::test]
async fn create_persists_consultation_with_ordered_questions() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let (view, questions) = svc
        .create(
            org,
            "Consulta de mobilidade",
            opens(),
            closes(),
            &prompts(3),
        )
        .await
        .expect("create");

    assert_eq!(view.org, org);
    assert_eq!(view.title, "Consulta de mobilidade");
    assert_eq!(questions.len(), 3);
    // Positions are the input order, 0-based.
    assert_eq!(questions[0].position, 0);
    assert_eq!(questions[2].position, 2);

    // The consultation row landed with status 'open' and the injected created_at.
    let row: (String, DateTime<Utc>) =
        sqlx::query_as("SELECT status, created_at FROM consultations_consultation WHERE id = $1")
            .bind(view.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(row.0, "open");
    assert_eq!(row.1, now());

    // Exactly three question rows, ordered by position.
    let positions: Vec<i32> = sqlx::query_scalar(
        "SELECT position FROM consultations_consultation_question \
         WHERE consultation_id = $1 ORDER BY position ASC",
    )
    .bind(view.id.as_uuid())
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(positions, vec![0, 1, 2]);
}

#[tokio::test]
async fn create_rejects_invalid_window_and_writes_nothing() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    // closes_at before opens_at => validation error, no transaction effect.
    let err = svc
        .create(org, "Janela inválida", closes(), opens(), &prompts(1))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE org_id = $1")
            .bind(org.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn create_rejects_empty_question_set() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let err = svc
        .create(org, "Sem perguntas", opens(), closes(), &[])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn create_rejects_blank_prompt() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let bad = vec!["Boa pergunta".to_owned(), "   ".to_owned()];
    let err = svc
        .create(org, "Prompt em branco", opens(), closes(), &bad)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));

    // The all-or-nothing transaction left no consultation behind.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE org_id = $1")
            .bind(org.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn close_transitions_open_to_closed_then_conflicts_on_repeat() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let (view, _) = svc
        .create(org, "Para encerrar", opens(), closes(), &prompts(1))
        .await
        .expect("create");

    // First close transitions the state.
    let closed = svc.close(org, view.id).await.expect("close");
    assert_eq!(
        closed.status,
        dsoc_consultations::ConsultationStatus::Closed
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM consultations_consultation WHERE id = $1")
            .bind(view.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(status, "closed");

    // A second close is rejected by the state guard (already closed) — Conflict, not a silent no-op.
    let err = svc.close(org, view.id).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));
}

#[tokio::test]
async fn close_unknown_consultation_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let err = svc.close(org, SpaceId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn close_is_scoped_by_org() {
    let db = connect().await;
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;
    let svc = service(db.clone());

    let (view, _) = svc
        .create(org_a, "Pertence a A", opens(), closes(), &prompts(1))
        .await
        .expect("create");

    // Org B cannot close org A's consultation — it is not found in B's scope.
    let err = svc.close(org_b, view.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));

    // And A's consultation is still open.
    let status: String =
        sqlx::query_scalar("SELECT status FROM consultations_consultation WHERE id = $1")
            .bind(view.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(status, "open");
}

#[tokio::test]
async fn get_returns_consultation_with_questions() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let (view, _) = svc
        .create(org, "Para ler", opens(), closes(), &prompts(2))
        .await
        .expect("create");

    let (got, questions) = svc.get(org, view.id).await.expect("get");
    assert_eq!(got.id, view.id);
    assert_eq!(got.title, "Para ler");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0].position, 0);

    // Missing id is NotFound.
    let err = svc.get(org, SpaceId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn list_is_keyset_paginated_and_org_scoped() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let other = seed_org(&db).await;
    let svc = service(db.clone());

    // Five consultations in `org`, one in `other` (must never appear in org's listing).
    let mut ids = Vec::new();
    for i in 0..5 {
        let (v, _) = svc
            .create(org, &format!("C{i}"), opens(), closes(), &prompts(1))
            .await
            .expect("create");
        ids.push(v.id);
    }
    svc.create(other, "Outro", opens(), closes(), &prompts(1))
        .await
        .expect("create other");

    // First page of 2, ascending by id (UUIDv7 ⇒ creation order).
    let (page1, total) = svc.list(org, None, 2).await.expect("page1");
    assert_eq!(total, 5);
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].id, ids[0]);
    assert_eq!(page1[1].id, ids[1]);

    // Next page continues after the last id of page 1.
    let (page2, _) = svc.list(org, Some(page1[1].id), 2).await.expect("page2");
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[0].id, ids[2]);
    assert_eq!(page2[1].id, ids[3]);
}

#[tokio::test]
async fn space_impl_reports_kind_components_and_open_state() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());
    let space = ConsultationSpace::new(db.clone());

    assert_eq!(space.kind(), "consultations");
    assert!(space.allows_component("surveys"));
    assert!(space.allows_component("comments"));
    assert!(!space.allows_component("votes"));

    let (view, _) = svc
        .create(org, "Espaço", opens(), closes(), &prompts(1))
        .await
        .expect("create");

    // An open consultation is open for participation.
    space.ensure_open(org, view.id).await.expect("open");

    // After closing, ensure_open reports a Conflict.
    svc.close(org, view.id).await.expect("close");
    let err = space.ensure_open(org, view.id).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));

    // A non-existent space is NotFound.
    let err = space.ensure_open(org, SpaceId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn directory_caller_creates_via_handler() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let state = app_state(db.clone(), VerificationLevel::Directory);

    let req = CreateConsultationRequest {
        title: "Via handler".to_owned(),
        opens_at: opens(),
        closes_at: closes(),
        questions: prompts(2),
    };
    let response = dsoc_consultations::http::create(State(state), caller(org), Json(req)).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE org_id = $1")
            .bind(org.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn email_caller_is_forbidden_by_create_handler() {
    let db = connect().await;
    let org = seed_org(&db).await;
    // Only email-verified: below the Directory floor for managing consultations.
    let state = app_state(db.clone(), VerificationLevel::Email);

    let req = CreateConsultationRequest {
        title: "Não autorizado".to_owned(),
        opens_at: opens(),
        closes_at: closes(),
        questions: prompts(1),
    };
    let response = dsoc_consultations::http::create(State(state), caller(org), Json(req)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The gate fired before any write.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE org_id = $1")
            .bind(org.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn close_and_read_handlers_round_trip() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());
    let (view, _) = svc
        .create(org, "Round trip", opens(), closes(), &prompts(1))
        .await
        .expect("create");

    // Read it through the handler.
    let state = app_state(db.clone(), VerificationLevel::Directory);
    let get_resp = dsoc_consultations::http::get_one(
        State(state.clone()),
        caller(org),
        Path(view.id.as_uuid()),
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::OK);

    // Close it through the handler.
    let close_resp =
        dsoc_consultations::http::close(State(state.clone()), caller(org), Path(view.id.as_uuid()))
            .await;
    assert_eq!(close_resp.status(), StatusCode::OK);

    // List through the handler returns the consultations for the org.
    let list_resp = dsoc_consultations::http::list(
        State(state),
        caller(org),
        Query(ListParams {
            after: None,
            limit: Some(10),
        }),
    )
    .await;
    assert_eq!(list_resp.status(), StatusCode::OK);

    let status: String =
        sqlx::query_scalar("SELECT status FROM consultations_consultation WHERE id = $1")
            .bind(view.id.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(status, "closed");
}

#[tokio::test]
async fn this_crate_emits_no_cross_crate_events() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let svc = service(db.clone());

    let (view, _) = svc
        .create(org, "Sem eventos", opens(), closes(), &prompts(1))
        .await
        .expect("create");
    svc.close(org, view.id).await.expect("close");

    // Per the Events policy, this crate writes nothing to the outbox (`events_log`).
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events_log WHERE org_id = $1")
        .bind(org.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(events, 0);
}
