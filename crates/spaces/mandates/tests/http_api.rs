//! HTTP-surface tests for `dsoc-mandates`. These drive the real Axum router returned by
//! [`dsoc_mandates::routes`] with `tower::ServiceExt::oneshot`, so the handlers, the
//! [`dsoc_app::CallerId`] authorization wiring (ADR-0007: the verified caller comes from the
//! trusted `x-dsoc-citizen-id` / `x-dsoc-org-id` headers, never the body), the DTO mapping, and the
//! `ApiErr` → status mapping are all exercised end-to-end against a real PostgreSQL
//! (docs/TESTING.md). A deterministic `FixedClock` is injected through `AppState`; nothing sleeps.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, MandateId, OrgId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;

/// Gateway-set headers carrying the verified caller (ADR-0007); the public ingress strips these
/// from client requests, so a handler can trust them as the authenticated identity.
const CITIZEN_HEADER: &str = "x-dsoc-citizen-id";
const ORG_HEADER: &str = "x-dsoc-org-id";

#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

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

fn state(db: Db, level: VerificationLevel) -> AppState {
    AppState {
        db,
        bus: Arc::new(RecordingEventBus::new()),
        authz: Arc::new(StubAuthz { level }),
        clock: Arc::new(FixedClock(now())),
    }
}

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

async fn seed_mandate(db: &Db, org: OrgId) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate \
         (id, org_id, office, display_name, public_email, is_candidate, onboarded_at, created_at) \
         VALUES ($1, $2, 'vereador', 'Vereadora Fulana', 'v@camara.test', false, NULL, $3)",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .bind(now())
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

/// Minimal percent-encoder for a query-string value (encodes the reserved characters that appear in
/// an RFC 3339 timestamp). Test-only; avoids pulling a URL crate into the dev dependencies.
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_owned(),
            '+' => "%2B".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

/// Read an Axum response into `(status, json_body)`.
async fn read(resp: axum::response::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn full_invite_accept_flow_over_http() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let caller = CitizenId::new();
    let app = dsoc_mandates::routes(state(db.clone(), VerificationLevel::Directory));

    // Invite (operator mutation: the verified caller comes from the gateway-set headers).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/invitations", mandate.as_uuid()))
                .header(CITIZEN_HEADER, caller.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["success"], json!(true));
    let token = body["data"]["token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 64);

    // Accept (authenticated by the token itself; no actor header).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/mandates/invitations/accept?org_id={}",
                    org.as_uuid()
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "token": token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["mandate_id"], json!(mandate.as_uuid()));

    // Read back: the contract DTO reports onboarded.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}?org_id={}",
                    mandate.as_uuid(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["onboarded"], json!(true));
    assert_eq!(body["data"]["office"], json!("vereador"));
}

#[tokio::test]
async fn invite_without_caller_headers_is_unauthorized() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    // No `x-dsoc-citizen-id` / `x-dsoc-org-id`: the `CallerId` extractor rejects with 401 before
    // the handler runs (ADR-0007 — the acting identity is never taken from the body/query).
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/invitations", mandate.as_uuid()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invite_with_insufficient_level_is_forbidden() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    // Caller resolves to Email — below Directory.
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Email));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/invitations", mandate.as_uuid()))
                .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], json!("forbidden"));
}

#[tokio::test]
async fn get_unknown_mandate_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}?org_id={}",
                    MandateId::new().as_uuid(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], json!("not_found"));
}

#[tokio::test]
async fn accept_unknown_token_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/mandates/invitations/accept?org_id={}",
                    org.as_uuid()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "token": format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple()) })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, _body) = read(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bind_identity_and_list_over_http() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/identity", mandate.as_uuid()))
                .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "level": "strong", "evidence_ref": "tse:abc" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["verification_level"], json!("strong"));

    // List the bindings back.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}/identity?org_id={}&limit=10",
                    mandate.as_uuid(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

/// Regression: the identity-binding list must paginate PAST the first page. The handler now passes
/// the composite `(after_at, after_id)` keyset cursor through to the service (it previously dropped
/// it and was stuck on page 1).
#[tokio::test]
async fn list_identity_bindings_paginates_with_composite_cursor() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    // Record three bindings.
    for level in ["email", "directory", "strong"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/mandates/{}/identity", mandate.as_uuid()))
                    .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
                    .header(ORG_HEADER, org.as_uuid().to_string())
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "level": level }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read(resp).await.0, StatusCode::CREATED);
    }

    // Page 1: the newest binding (the list is ordered newest-first).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}/identity?org_id={}&limit=1",
                    mandate.as_uuid(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, page1) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    let page1 = page1["data"].as_array().unwrap();
    assert_eq!(page1.len(), 1);
    let cursor_at = page1[0]["verified_at"].as_str().unwrap().to_owned();
    let cursor_id = page1[0]["id"].as_str().unwrap().to_owned();

    // Page 2: hand back the composite cursor; we must get a DIFFERENT (older) binding, proving the
    // cursor reached the service rather than being discarded.
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}/identity?org_id={}&limit=1&after_at={}&after_id={}",
                    mandate.as_uuid(),
                    org.as_uuid(),
                    urlencode(&cursor_at),
                    cursor_id,
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, page2) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    let page2 = page2["data"].as_array().unwrap();
    assert_eq!(page2.len(), 1);
    assert_ne!(
        page2[0]["id"].as_str().unwrap(),
        cursor_id,
        "page 2 must advance past the page-1 cursor"
    );
}

#[tokio::test]
async fn bind_identity_with_bad_level_is_unprocessable() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/identity", mandate.as_uuid()))
                .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(json!({ "level": "emperor" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], json!("invalid_input"));
}

#[tokio::test]
async fn add_office_and_list_over_http() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let app = dsoc_mandates::routes(state(db, VerificationLevel::Directory));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/mandates/{}/offices", mandate.as_uuid()))
                .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "office": "prefeito",
                        "district": "Centro",
                        "term_start": "2025-01-01",
                        "term_end": "2028-12-31"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["office"], json!("prefeito"));

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/mandates/{}/offices?org_id={}&limit=10",
                    mandate.as_uuid(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}
