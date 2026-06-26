//! HTTP-surface tests for `dsoc-assemblies`. These drive the real Axum router returned by
//! [`dsoc_assemblies::routes`] with `tower::ServiceExt::oneshot`, so the handlers, the
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
use dsoc_core::ids::{CitizenId, OrgId};
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
        storage: None,
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

/// Parse a response body into JSON.
async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn create_then_add_and_list_members_full_flow() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let app = dsoc_assemblies::routes(state(db.clone(), VerificationLevel::Email));

    // Create the assembly (org/caller come from the trusted headers, not the body).
    let req = Request::builder()
        .method("POST")
        .uri("/assemblies")
        .header("content-type", "application/json")
        .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
        .header(ORG_HEADER, org.as_uuid().to_string())
        .body(Body::from(json!({ "name": "Assembleia" }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp.into_body()).await;
    assert_eq!(created["success"], json!(true));
    let assembly_id = created["data"]["id"].as_str().unwrap().to_owned();

    // Add a member (the citizen in the body is the member; the caller header is the organizer).
    let req = Request::builder()
        .method("POST")
        .uri(format!("/assemblies/{assembly_id}/members"))
        .header("content-type", "application/json")
        .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
        .header(ORG_HEADER, org.as_uuid().to_string())
        .body(Body::from(
            json!({ "citizen_id": citizen.as_uuid(), "role": "chair" }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let added = body_json(resp.into_body()).await;
    assert_eq!(added["data"]["role"], json!("chair"));

    // List the roster.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/assemblies/{assembly_id}/members?limit=10"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_json(resp.into_body()).await;
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    // Get the assembly by id (read path; org via query string).
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/assemblies/{assembly_id}?org_id={}",
            org.as_uuid()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = body_json(resp.into_body()).await;
    assert_eq!(got["data"]["name"], json!("Assembleia"));
}

#[tokio::test]
async fn create_without_caller_headers_is_unauthorized() {
    let db = connect().await;
    let _org = seed_org(&db).await;
    let app = dsoc_assemblies::routes(state(db, VerificationLevel::Email));

    // No x-dsoc-citizen-id / x-dsoc-org-id headers: the CallerId extractor rejects the request.
    let req = Request::builder()
        .method("POST")
        .uri("/assemblies")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "name": "Assembleia" }).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_with_insufficient_level_is_forbidden() {
    let db = connect().await;
    let org = seed_org(&db).await;
    // Anonymous caller is below the Email mutation bar.
    let app = dsoc_assemblies::routes(state(db, VerificationLevel::Anonymous));

    let req = Request::builder()
        .method("POST")
        .uri("/assemblies")
        .header("content-type", "application/json")
        .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
        .header(ORG_HEADER, org.as_uuid().to_string())
        .body(Body::from(json!({ "name": "Assembleia" }).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_member_to_unknown_assembly_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let app = dsoc_assemblies::routes(state(db, VerificationLevel::Email));

    let req = Request::builder()
        .method("POST")
        .uri(format!("/assemblies/{}/members", Uuid::now_v7()))
        .header("content-type", "application/json")
        .header(CITIZEN_HEADER, CitizenId::new().as_uuid().to_string())
        .header(ORG_HEADER, org.as_uuid().to_string())
        .body(Body::from(
            json!({ "citizen_id": citizen.as_uuid() }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_unknown_assembly_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let app = dsoc_assemblies::routes(state(db, VerificationLevel::Email));

    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/assemblies/{}?org_id={}",
            Uuid::now_v7(),
            org.as_uuid()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["success"], json!(false));
    assert_eq!(body["error"]["code"], json!("not_found"));
}
