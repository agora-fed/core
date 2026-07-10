//! HTTP-surface integration tests for the gateway against a real PostgreSQL
//! (TESTING.md: no mocked DB). One `oneshot` request per assertion exercises
//! the full stack: router composition, the `inject_identity` middleware, the
//! handler, and the SQL underneath.
//!
//! Split mirrors the platform's testing policy:
//! - SECURITY: authentication/authorization gates, input validation, and
//!   information-leak checks (anonymous callers must see 401, non-admins 403,
//!   invalid input 400 — never a 500 and never data).
//! - FUNCTIONAL: the public read surface and the citizen-preference loop.
//!
//! Requires `DATABASE_URL` with the full migration chain applied (CI does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use tower::util::ServiceExt;
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;

async fn connect() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a test database");
    dsoc_db::connect(&url, 5).await.expect("connect")
}

async fn state() -> AppState {
    let db = connect().await;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let bus = Arc::new(dsoc_events::PgEventBus::new(db.clone()));
    let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
    AppState {
        db,
        bus,
        authz,
        clock,
        storage: None,
    }
}

async fn app() -> (Router, AppState) {
    let st = state().await;
    (dsoc_gateway::api_router(st.clone()), st)
}

/// Seed an isolated org + citizen + live session; returns the session cookie value.
async fn seed_session(db: &Db) -> (Uuid, Uuid, String) {
    let org = Uuid::now_v7();
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    let now = Utc::now();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Test Org', $3)")
        .bind(org)
        .bind(format!("org-{}", org.simple()))
        .bind(now)
        .execute(db)
        .await
        .expect("seed org");
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at)
         VALUES ($1, $2, $3, 'email', $4)",
    )
    .bind(citizen)
    .bind(org)
    .bind(format!("sub-{}", citizen.simple()))
    .bind(now)
    .execute(db)
    .await
    .expect("seed citizen");
    sqlx::query(
        "INSERT INTO auth_session (id, org_id, citizen_id, oidc_subject, issued_at,
                                   expires_at, public_handle, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $5)",
    )
    .bind(session)
    .bind(org)
    .bind(citizen)
    .bind(format!("sub-{}", citizen.simple()))
    .bind(now)
    .bind(now + Duration::hours(1))
    .bind(format!("u-{}", citizen.simple()))
    .execute(db)
    .await
    .expect("seed session");
    (org, citizen, format!("dsoc_session={session}"))
}

async fn grant_admin(db: &Db, org: Uuid, citizen: Uuid) {
    sqlx::query(
        "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
         VALUES ($1, $2, $3, 'admin', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(citizen)
    .bind(Utc::now())
    .execute(db)
    .await
    .expect("grant admin");
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn json_req(method: &str, uri: &str, cookie: Option<&str>, body: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::from(body.to_owned())).unwrap()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// SECURITY — authentication gates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anonymous_cannot_read_preferences() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/me/preferences")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn anonymous_cannot_export_lgpd_data() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/me/lgpd/export")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn forged_session_cookie_is_anonymous() {
    // A syntactically valid but unknown session id must not resolve to anyone.
    let (app, _) = app().await;
    let resp = app
        .oneshot(get_with_cookie(
            "/api/v1/me/preferences",
            &format!("dsoc_session={}", Uuid::now_v7()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let (app, st) = app().await;
    let (org, citizen, _) = seed_session(&st.db).await;
    let stale = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO auth_session (id, org_id, citizen_id, oidc_subject, issued_at,
                                   expires_at, public_handle, created_at)
         VALUES ($1, $2, $3, 'sub-stale', now() - interval '2 hours',
                 now() - interval '1 hour', 'u-stale', now() - interval '2 hours')",
    )
    .bind(stale)
    .bind(org)
    .bind(citizen)
    .execute(&st.db)
    .await
    .unwrap();
    let resp = app
        .oneshot(get_with_cookie(
            "/api/v1/me/preferences",
            &format!("dsoc_session={stale}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// SECURITY — authorization (admin) gates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_rules_crud_requires_admin_role() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    // Anonymous → 401.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/rules",
            None,
            r#"{"text":"x"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Authenticated but NOT admin → 403, and no rule row is created.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/rules",
            Some(&cookie),
            r#"{"text":"regra intrusa"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_rule WHERE text = 'regra intrusa'")
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn admin_webhooks_listing_is_gated() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(get("/api/v1/admin/webhooks"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = app
        .oneshot(get_with_cookie("/api/v1/admin/webhooks", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_role_unlocks_rules_crud() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/rules",
            Some(&cookie),
            r#"{"text":"Respeite o próximo.","ordinal":1}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["success"], serde_json::Value::Bool(true));
}

// ---------------------------------------------------------------------------
// SECURITY — input validation at the boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preferences_reject_invalid_visibility() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "PATCH",
            "/api/v1/me/preferences",
            Some(&cookie),
            r#"{"default_visibility":"world-readable"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn politicos_browse_requires_sphere() {
    // Without the mandatory sphere filter the endpoint must refuse, not dump
    // the entire 70k-mandate table.
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/politicos/browse")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oauth_token_rejects_unknown_authorization_code() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/oauth/token",
            None,
            &format!(
                r#"{{"grant_type":"authorization_code","code":"{}","client_id":"{}","client_secret":"x","redirect_uri":"urn:ietf:wg:oauth:2.0:oob"}}"#,
                Uuid::now_v7(),
                Uuid::now_v7()
            ),
        ))
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "bogus code must be a 4xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn govbr_is_dormant_without_credentials() {
    // With no GOVBR_CLIENT_ID in the env the status endpoint must say disabled
    // and the start endpoint must NOT redirect anywhere.
    let (app, _) = app().await;
    let resp = app
        .clone()
        .oneshot(get("/api/v1/auth/govbr/status"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["enabled"], serde_json::Value::Bool(false));
    let resp = app.oneshot(get("/auth/govbr/start")).await.unwrap();
    assert_ne!(resp.status(), StatusCode::FOUND);
}

// ---------------------------------------------------------------------------
// FUNCTIONAL — public read surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_is_public_and_ok() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn public_stats_expose_no_pii() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/stats/public")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["success"], serde_json::Value::Bool(true));
    let rendered = body.to_string();
    assert!(
        !rendered.contains("@"),
        "public stats must not leak e-mails"
    );
}

#[tokio::test]
async fn elections_2026_are_seeded_and_public() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/elections")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let list = body["data"].as_array().expect("elections array");
    // Migration 0505 seeds the 4 structural 2026 rows (fed/est × R1/R2).
    assert!(
        list.len() >= 4,
        "expected the 0505 seed, got {}",
        list.len()
    );
}

#[tokio::test]
async fn parties_catalogue_is_public() {
    let (app, _) = app().await;
    // org_id is mandatory (multi-org surface); a missing one must 400, the
    // default org must answer with the party array.
    let resp = app.clone().oneshot(get("/api/v1/parties")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = app
        .oneshot(get(
            "/api/v1/parties?org_id=11111111-1111-1111-1111-111111111111",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["success"], serde_json::Value::Bool(true));
    assert!(body["data"].is_array());
}

#[tokio::test]
async fn server_rules_are_public() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/server/rules")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn openapi_document_is_served() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/openapi.json")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// FUNCTIONAL — citizen preference loop (incl. 0.26.24 auto_federate_threshold)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preferences_roundtrip_with_auto_federate_opt_out() {
    let (app, st) = app().await;
    let (_, citizen, cookie) = seed_session(&st.db).await;

    // Defaults: everything on, auto-federation opt-out included in the DTO.
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/preferences", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["data"]["auto_federate_threshold"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(body["data"]["default_visibility"], "public");

    // Opt out; the column must flip and the GET must reflect it.
    let resp = app
        .clone()
        .oneshot(json_req(
            "PATCH",
            "/api/v1/me/preferences",
            Some(&cookie),
            r#"{"auto_federate_threshold":false,"default_sensitive":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let flag: bool =
        sqlx::query_scalar("SELECT auto_federate_threshold FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(!flag);
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/preferences", &cookie))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["data"]["auto_federate_threshold"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(
        body["data"]["default_sensitive"],
        serde_json::Value::Bool(true)
    );
}

#[tokio::test]
async fn notifications_feed_is_empty_for_fresh_citizen() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/notifications", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    // Envelope: { items: [], unread_count: 0 } for a citizen nobody touched yet.
    assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["data"]["unread_count"], serde_json::json!(0));
}
