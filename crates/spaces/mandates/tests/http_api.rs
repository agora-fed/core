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

// ---------------------------------------------------------------------------
// Party directory write surface (0.37.0 — Fase 2.1)
// ---------------------------------------------------------------------------

async fn seed_citizen(db: &Db, org: OrgId) -> CitizenId {
    let c = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at) \
         VALUES ($1, $2, 'directory', $3)",
    )
    .bind(c.as_uuid())
    .bind(org.as_uuid())
    .bind(now())
    .execute(db)
    .await
    .expect("seed citizen");
    c
}

async fn seed_citizen_with_handle(db: &Db, org: OrgId, handle: &str) -> CitizenId {
    let c = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, handle, created_at) \
         VALUES ($1, $2, 'directory', $3, $4)",
    )
    .bind(c.as_uuid())
    .bind(org.as_uuid())
    .bind(handle)
    .bind(now())
    .execute(db)
    .await
    .expect("seed citizen with handle");
    c
}

async fn seed_party(db: &Db, org: OrgId, sigla: &str) {
    sqlx::query("INSERT INTO party (org_id, sigla, name) VALUES ($1, $2, $2)")
        .bind(org.as_uuid())
        .bind(sigla)
        .execute(db)
        .await
        .expect("seed party");
}

async fn grant_platform_admin(db: &Db, org: OrgId, citizen: CitizenId) {
    sqlx::query(
        "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at) \
         VALUES ($1, $2, $3, 'admin', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(org.as_uuid())
    .bind(citizen.as_uuid())
    .bind(now())
    .execute(db)
    .await
    .expect("grant admin");
}

fn parties_app(db: Db) -> axum::Router {
    dsoc_mandates::parties_routes(state(db, VerificationLevel::Directory))
}

#[tokio::test]
async fn platform_admin_creates_municipal_directory_and_lists_members() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;
    let responsavel = seed_citizen(&db, org).await;

    // Cria o diretório municipal do PT em Porto Alegre/RS, já com responsável.
    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, admin.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "org_id": org.as_uuid(),
                        "esfera": "municipal",
                        "uf": "rs",
                        "municipio": "Porto Alegre",
                        "name": "Diretório Municipal do PT — Porto Alegre",
                        "responsavel_citizen_id": responsavel.as_uuid()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    let dir_id = body["data"]["id"].as_str().unwrap().to_owned();

    // O responsável nasce junto: vínculo admin no escopo do diretório recém-criado.
    let (resp_citizen, resp_role): (Uuid, String) = sqlx::query_as(
        "SELECT citizen_id, role FROM party_administrator \
         WHERE org_id = $1 AND party_sigla = 'PT' AND directory_id = $2::uuid",
    )
    .bind(org.as_uuid())
    .bind(Uuid::parse_str(&dir_id).unwrap())
    .fetch_one(&db)
    .await
    .expect("responsável gravado na criação");
    assert_eq!(resp_citizen, responsavel.as_uuid());
    assert_eq!(resp_role, "admin");

    // Dois vereadores do PT em Porto Alegre + um de outra cidade (não deve entrar).
    for (name, city) in [
        ("Vereador A", "Porto Alegre"),
        ("Vereadora B", "Porto Alegre"),
        ("Vereador C", "Canoas"),
    ] {
        sqlx::query(
            "INSERT INTO mandate (id, org_id, office, display_name, public_email, is_candidate, \
             created_at, party, uf, sphere, municipio) \
             VALUES ($1, $2, 'vereador', $3, 'x@camara.test', false, $4, 'PT', 'RS', 'municipal', $5)",
        )
        .bind(Uuid::now_v7())
        .bind(org.as_uuid())
        .bind(name)
        .bind(now())
        .bind(city)
        .execute(&db)
        .await
        .unwrap();
    }

    // Membros derivados: só os dois de Porto Alegre.
    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/parties/PT/directories/{dir_id}/members?org_id={}",
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    let members = body["data"].as_array().unwrap();
    assert_eq!(members.len(), 2, "só os vereadores de Porto Alegre: {body}");
    assert_eq!(members[0]["display_name"], json!("Vereador A"));
}

#[tokio::test]
async fn non_admin_cannot_create_directory() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let stranger = seed_citizen(&db, org).await; // sem admin_role_binding / party_administrator

    let app = parties_app(db);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, stranger.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "org_id": org.as_uuid(),
                        "esfera": "estadual",
                        "uf": "RS",
                        "name": "Diretório intruso"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_directory_without_caller_is_unauthorized() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let app = parties_app(db);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"org_id": org.as_uuid(), "esfera": "federal", "name": "x"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn municipal_directory_rejects_missing_municipio() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;

    let app = parties_app(db);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, admin.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"org_id": org.as_uuid(), "esfera": "municipal", "uf": "RS", "name": "x"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("federative_shape"));
}

#[tokio::test]
async fn create_directory_without_responsavel_is_rejected() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;

    let app = parties_app(db);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, admin.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "org_id": org.as_uuid(),
                        "esfera": "estadual",
                        "uf": "RS",
                        "name": "Diretório sem responsável"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["error"]["code"], json!("missing_responsavel"));
}

#[tokio::test]
async fn create_directory_resolves_responsavel_by_handle() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;
    let responsavel = seed_citizen_with_handle(&db, org, "maria_dirigente").await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, admin.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "org_id": org.as_uuid(),
                        "esfera": "estadual",
                        "uf": "RS",
                        "name": "Diretório Estadual do PT — RS",
                        "responsavel_handle": "@maria_dirigente"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    let dir_id = Uuid::parse_str(body["data"]["id"].as_str().unwrap()).unwrap();

    let citizen: Uuid = sqlx::query_scalar(
        "SELECT citizen_id FROM party_administrator \
         WHERE org_id = $1 AND party_sigla = 'PT' AND directory_id = $2 AND role = 'admin'",
    )
    .bind(org.as_uuid())
    .bind(dir_id)
    .fetch_one(&db)
    .await
    .expect("responsável resolvido por handle");
    assert_eq!(citizen, responsavel.as_uuid());
}

#[tokio::test]
async fn create_directory_with_unknown_responsavel_creates_nothing() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/parties/PT/directories")
                .header(CITIZEN_HEADER, admin.as_uuid().to_string())
                .header(ORG_HEADER, org.as_uuid().to_string())
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "org_id": org.as_uuid(),
                        "esfera": "estadual",
                        "uf": "RS",
                        "name": "Diretório fantasma",
                        "responsavel_handle": "@nao_existe"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={body}");
    assert_eq!(body["error"]["code"], json!("responsavel_not_found"));

    // Transação única: sem responsável válido, o diretório NÃO pode ter nascido.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM party_directory WHERE org_id = $1 AND name = 'Diretório fantasma'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(count, 0, "diretório órfão criado apesar do 404");
}

/// Índice territorial 0673 (incidente PT-Ubatuba): o mesmo território do mesmo
/// partido não pode ter dois diretórios — a segunda criação responde 409, mesmo
/// com nome diferente (território é a identidade, não o rótulo).
#[tokio::test]
async fn duplicate_territorial_directory_is_conflict() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let admin = seed_citizen(&db, org).await;
    grant_platform_admin(&db, org, admin).await;
    let responsavel = seed_citizen(&db, org).await;

    let payload = |name: &str| {
        json!({
            "org_id": org.as_uuid(),
            "esfera": "municipal",
            "uf": "SP",
            "municipio": "Ubatuba",
            "name": name,
            "responsavel_citizen_id": responsavel.as_uuid()
        })
        .to_string()
    };
    let req = |body: String| {
        Request::builder()
            .method("POST")
            .uri("/parties/PT/directories")
            .header(CITIZEN_HEADER, admin.as_uuid().to_string())
            .header(ORG_HEADER, org.as_uuid().to_string())
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    };

    let resp = parties_app(db.clone())
        .oneshot(req(payload("Diretório Municipal PT-Ubatuba")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = parties_app(db.clone())
        .oneshot(req(payload("Diretório Municipal PT-Ubatuba (de novo)")))
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_eq!(body["error"]["code"], json!("directory_exists"));

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM party_directory \
         WHERE org_id = $1 AND party_sigla = 'PT' AND municipio = 'Ubatuba'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(count, 1, "duplicata territorial persistida");
}

// ---------------------------------------------------------------------------
// CHAPTERS — public chapter page payload (EN contract, ADR-0013)
// ---------------------------------------------------------------------------

/// Seed a municipal directory + one ACCEPTED chapter-scoped admin directly in
/// SQL (the creation flow is covered elsewhere; here we pin the READ contract).
async fn seed_chapter(db: &Db, org: OrgId, sigla: &str) -> (Uuid, CitizenId) {
    let dir_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO party_directory (id, org_id, party_sigla, esfera, uf, municipio, name) \
         VALUES ($1, $2, $3, 'municipal', 'sp', 'Ubatuba', 'Diretório Municipal — Ubatuba')",
    )
    .bind(dir_id)
    .bind(org.as_uuid())
    .bind(sigla)
    .execute(db)
    .await
    .expect("seed directory");
    let admin = seed_citizen(db, org).await;
    sqlx::query("UPDATE citizen SET handle = 'dir-admin' WHERE id = $1")
        .bind(admin.as_uuid())
        .execute(db)
        .await
        .expect("set handle");
    sqlx::query(
        "INSERT INTO party_administrator \
             (id, org_id, party_sigla, directory_id, citizen_id, role, accepted_at) \
         VALUES ($1, $2, $3, $4, $5, 'admin', $6)",
    )
    .bind(Uuid::now_v7())
    .bind(org.as_uuid())
    .bind(sigla)
    .bind(dir_id)
    .bind(admin.as_uuid())
    .bind(now())
    .execute(db)
    .await
    .expect("seed chapter admin");
    (dir_id, admin)
}

#[tokio::test]
async fn chapter_payload_is_english_and_privacy_safe() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let (dir_id, admin) = seed_chapter(&db, org, "PT").await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/parties/PT/chapters/{dir_id}?org_id={}",
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    // English contract (ADR-0013): level/state/municipality, esfera mapped.
    let data = &body["data"];
    assert_eq!(data["party_short_name"], "PT");
    assert_eq!(data["level"], "municipal");
    assert_eq!(data["state"], "SP");
    assert_eq!(data["municipality"], "Ubatuba");
    assert_eq!(data["name"], "Diretório Municipal — Ubatuba");
    assert_eq!(data["administrators"][0]["public_handle"], "dir-admin");
    assert_eq!(data["administrators"][0]["role"], "admin");

    // SECURITY / privacy wall: the raw payload must never carry the admin's
    // citizen UUID nor any e-mail (AdminBriefDto contract).
    let raw = body.to_string();
    assert!(
        !raw.contains(&admin.as_uuid().to_string()),
        "citizen UUID leaked in chapter payload"
    );
    assert!(
        !raw.contains('@') || raw.contains("\"@"),
        "e-mail-like leak: {raw}"
    );
}

#[tokio::test]
async fn chapter_unknown_id_is_null_not_error() {
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/parties/PT/chapters/{}?org_id={}",
                    Uuid::now_v7(),
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_null(), "body={body}");
}

#[tokio::test]
async fn chapter_under_wrong_party_is_null() {
    // A REAL chapter id requested under another party's sigla must be a miss
    // (no cross-party enumeration through ids).
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    seed_party(&db, org, "PSOL").await;
    let (dir_id, _) = seed_chapter(&db, org, "PT").await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/parties/PSOL/chapters/{dir_id}?org_id={}",
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_null(), "body={body}");
}

#[tokio::test]
async fn chapter_sigla_injection_is_inert() {
    // SECURITY: sigla arrives as a bound parameter; a classic injection
    // payload must behave as a plain (missing) sigla, never touch the query.
    let db = connect().await;
    let org = seed_org(&db).await;
    seed_party(&db, org, "PT").await;
    let (dir_id, _) = seed_chapter(&db, org, "PT").await;

    let app = parties_app(db.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/parties/PT%27%20OR%20%271%27%3D%271/chapters/{dir_id}?org_id={}",
                    org.as_uuid()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = read(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_null(), "injection must be a miss: {body}");
}
