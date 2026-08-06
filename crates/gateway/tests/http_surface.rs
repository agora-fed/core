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
    seed_session_in_org(db, Uuid::now_v7()).await
}

/// Same as [`seed_session`] but pinning the org — the federation surface
/// resolves handles against the fixed default org, so federated tests
/// precisam seedar NELA (idempotente via ON CONFLICT).
async fn seed_session_in_org(db: &Db, org: Uuid) -> (Uuid, Uuid, String) {
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Test Org', $3)
         ON CONFLICT (id) DO NOTHING",
    )
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
async fn anonymous_cannot_whoami() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/me/whoami")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// `GET /me/whoami` (mobile): an ordinary logged-in citizen returns civic_type=cidadao,
/// with no admin/party role and no mandate. Verifies the consolidated composition.
#[tokio::test]
async fn whoami_for_plain_citizen_is_cidadao() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/whoami", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["data"]["civic_type"], "cidadao");
    assert_eq!(v["data"]["is_admin"], false);
    assert_eq!(v["data"]["verification_level"], "email");
    assert!(v["data"]["mandate"].is_null());
    assert!(v["data"]["platform_role"].is_null());
}

/// Admin logado: whoami reflete platform_role=owner/admin e is_admin=true.
#[tokio::test]
async fn whoami_flags_admin() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/whoami", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["data"]["is_admin"], true);
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

/// SECURITY (2026-07-24 regression): the caller's identity comes from the headers
/// `x-dsoc-citizen-id`/`x-dsoc-org-id`/`x-citizen-id`, which `inject_identity`
/// may only set from a REAL session/bearer. A client injecting them directly
/// (with no cookie) must NOT be accepted — otherwise it impersonates any
/// citizen, admins included. Before the fix this request returned 200 with the
/// admin stats; now the headers are stripped and it falls to 401.
#[tokio::test]
async fn spoofed_identity_headers_are_stripped() {
    let (app, st) = app().await;
    let (org, citizen, _cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    // No cookie, but forging the identity headers of the freshly created admin.
    let req = Request::builder()
        .uri("/api/v1/admin/stats")
        .header("x-dsoc-citizen-id", citizen.to_string())
        .header("x-citizen-id", citizen.to_string())
        .header("x-dsoc-org-id", org.to_string())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "headers de identidade forjados pelo cliente devem ser ignorados"
    );
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

// ---------------------------------------------------------------------------
// SECURITY — 0.28.x surface: contact form, attestations, signup gates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contact_rejects_unknown_sector() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/contact",
            None,
            r#"{"sector":"marketing","name":"Nome","email":"a@b.co","subject":"assunto","message":"mensagem com tamanho ok"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn contact_honeypot_pretends_success_and_sends_nothing() {
    // A bot filling the hidden field gets a 200 "ok" — no SMTP, no effect.
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/contact",
            None,
            r#"{"sector":"contato","name":"Bot","email":"bot@spam.co","subject":"spam","message":"mensagem de robô com tamanho","website":"http://spam.example"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn contact_rate_limits_per_ip() {
    // 5/h per IP (default). The 6th from the same IP must see 429 — before
    // any SMTP attempt.
    let (app, _) = app().await;
    let body = r#"{"sector":"contato","name":"Nome","email":"a@b.co","subject":"assunto","message":"mensagem com tamanho ok"}"#;
    for _ in 0..5 {
        let mut req = json_req("POST", "/api/v1/contact", None, body);
        req.headers_mut()
            .insert("x-forwarded-for", "198.51.100.77".parse().unwrap());
        let _ = app.clone().oneshot(req).await.unwrap();
    }
    let mut req = json_req("POST", "/api/v1/contact", None, body);
    req.headers_mut()
        .insert("x-forwarded-for", "198.51.100.77".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn attest_requires_session() {
    let (app, st) = app().await;
    let (_, citizen, _) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/citizens/{citizen}/attestations"),
            None,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn attest_rejects_self_attestation() {
    let (app, st) = app().await;
    let (_, citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/citizens/{citizen}/attestations"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn attest_requires_verified_operator_power() {
    // An ordinary session (no mandate, no party) cannot attest anyone.
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let (_, target, _) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/citizens/{target}/attestations"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn signup_gates_admin_surface_is_gated() {
    let (app, st) = app().await;
    // Anonymous: 401.
    let resp = app
        .clone()
        .oneshot(get("/api/v1/admin/email_domain_blocks"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Ordinary session: 403 — never the list.
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie(
            "/api/v1/admin/email_domain_blocks",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn blocked_email_domain_gates_register() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    // The admin blocks the domain through the real API.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/email_domain_blocks",
            Some(&cookie),
            r#"{"domain":"blocked-gate-test.example","reason":"teste"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Signup with an e-mail from the blocked domain: an opaque 403 from the gate.
    let register = format!(
        r#"{{"org_id":"{org}","email":"x@blocked-gate-test.example","password":"senha-forte-123","cpf":"00000000000"}}"#
    );
    let resp = app
        .clone()
        .oneshot(json_req("POST", "/api/v1/auth/register", None, &register))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // With the rule removed, the same request falls back to normal validation
    // (an invalid document = a 4xx validation error, never the gate's 403).
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            "/api/v1/admin/email_domain_blocks/blocked-gate-test.example",
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req("POST", "/api/v1/auth/register", None, &register))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn ip_deny_rule_gates_login_scope() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/ip_rules",
            Some(&cookie),
            r#"{"cidr":"198.51.100.0/24","scope":"login","state":"deny","reason":"teste"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let login =
        format!(r#"{{"org_id":"{org}","email":"ninguem@example.org","password":"whatever"}}"#);
    // An IP inside the deny range: the gate's 403, before any credential check.
    let mut req = json_req("POST", "/api/v1/auth/login", None, &login);
    req.headers_mut()
        .insert("x-forwarded-for", "198.51.100.9".parse().unwrap());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // IP outside the deny: passes the gate and lands on the normal 401 for bad credentials.
    // A unique IP per run — the attempt audit persists across executions
    // and a fixed IP would hit the rate limit (429) on the nth round.
    let b = Uuid::now_v7();
    let b = b.as_bytes();
    let outside_ip = format!("10.{}.{}.{}", b[13], b[14], b[15]);
    let mut req = json_req("POST", "/api/v1/auth/login", None, &login);
    req.headers_mut()
        .insert("x-forwarded-for", outside_ip.parse().unwrap());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Cleanup: the rule is global — remove it so it never leaks into other tests.
    let rules = body_json(
        app.clone()
            .oneshot(get_with_cookie("/api/v1/admin/ip_rules", &cookie))
            .await
            .unwrap(),
    )
    .await;
    let id = rules["data"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|r| r["cidr"] == "198.51.100.0/24" && r["scope"] == "login")
        })
        .and_then(|r| r["id"].as_str())
        .expect("rule id")
        .to_owned();
    let resp = app
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/admin/ip_rules/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ip_rule_rejects_invalid_cidr() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/ip_rules",
            Some(&cookie),
            r#"{"cidr":"not-a-cidr","scope":"login","state":"deny"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// FUNCTIONAL — fediverso public reads + attestation loop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webfinger_without_resource_is_client_error() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/.well-known/webfinger")).await.unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());
}

#[tokio::test]
async fn webfinger_unknown_account_is_client_error() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(get(
            "/.well-known/webfinger?resource=acct:nobody-xyz@localhost",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());
}

#[tokio::test]
async fn unknown_actor_is_client_error_for_activitypub() {
    // A missing/invalid handle never returns 200 nor 500 to an AP peer —
    // the surface answers 4xx (400 for an invalid shape, 404 for an absent one).
    let (app, _) = app().await;
    let req = Request::builder()
        .uri("/actors/does-not-exist-xyz")
        .header(header::ACCEPT, "application/activity+json")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());
}

#[tokio::test]
async fn attestations_public_list_starts_empty() {
    let (app, st) = app().await;
    let (_, citizen, _) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get(&format!("/api/v1/citizens/{citizen}/attestations")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["count"], 0);
    assert_eq!(json["data"]["viewer_can_attest"], false);
}

#[tokio::test]
async fn attest_and_revoke_roundtrip() {
    let (app, st) = app().await;
    // Attester: a mandate operator (verified binding).
    let (org, attester, cookie) = seed_session(&st.db).await;
    let mandate = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at)
         VALUES ($1, $2, 'deputado_federal', 'Mandato Teste', 'gab@example.leg.br', now())",
    )
    .bind(mandate)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed mandate");
    sqlx::query(
        "INSERT INTO mandate_identity_binding
             (id, mandate_id, citizen_id, verification_level, verified_at, created_at)
         VALUES ($1, $2, $3, 'directory', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(mandate)
    .bind(attester)
    .execute(&st.db)
    .await
    .expect("seed binding");
    let (_, target, _) = seed_session(&st.db).await;

    // Attest with a note.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/citizens/{target}/attestations"),
            Some(&cookie),
            r#"{"note":"conheço do trabalho de base"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The public list shows 1 + the logged-in viewer's flags.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/citizens/{target}/attestations"),
            &cookie,
        ))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["data"]["count"], 1);
    assert_eq!(json["data"]["viewer_attested"], true);
    assert_eq!(json["data"]["items"][0]["kind"], "mandato");

    // Revoga; a lista volta a zero.
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/citizens/{target}/attestations"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(get(&format!("/api/v1/citizens/{target}/attestations")))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["data"]["count"], 0);
}

#[tokio::test]
async fn mastodon_verify_credentials_requires_auth() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(get("/api/v1/accounts/verify_credentials"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bookmarks_list_empty_for_fresh_session() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/bookmarks", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// FUNCTIONAL — mastodon API + federation actor surface (issue #8, passo 2)
// ---------------------------------------------------------------------------

/// Turn the session's citizen into a public, federable profile (handle + is_public).
/// Handle respeita o CHECK `citizen_handle_format` (3–32 chars).
async fn make_public(db: &Db, citizen: Uuid) -> String {
    let simple = citizen.simple().to_string();
    let handle = format!("h{}", &simple[..12]);
    sqlx::query("UPDATE citizen SET handle = $2, is_public = true, display_name = 'Perfil Teste' WHERE id = $1")
        .bind(citizen)
        .bind(&handle)
        .execute(db)
        .await
        .expect("make public");
    handle
}

#[tokio::test]
async fn instance_metadata_is_public() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/instance")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn public_timeline_is_readable_anonymously() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/timelines/public")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn home_timeline_requires_auth() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/timelines/home")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_status_requires_auth() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/statuses",
            None,
            r#"{"status":"olá mundo"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn publish_status_and_serve_actor_documents() {
    // Full federated flow: a public profile publishes a note through the
    // Mastodon-compatible API, and the ActivityPub surface serves actor/outbox/followers.
    let (app, st) = app().await;
    // The federated surface's handle resolution uses the fixed default org.
    let default_org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let (_, citizen, cookie) = seed_session_in_org(&st.db, default_org).await;
    let handle = make_public(&st.db, citizen).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/statuses",
            Some(&cookie),
            r#"{"status":"nota de teste da suíte de cobertura"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["id"].is_string() || json["data"]["id"].is_string());

    for path in [
        format!("/actors/{handle}"),
        format!("/actors/{handle}/outbox"),
        format!("/actors/{handle}/followers"),
        format!("/actors/{handle}/following"),
    ] {
        let req = Request::builder()
            .uri(&path)
            .header(header::ACCEPT, "application/activity+json")
            // O handler monta URLs absolutas do actor a partir do Host.
            .header(header::HOST, "localhost")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    }
}

#[tokio::test]
async fn apps_registration_and_bad_oauth_token() {
    let (app, _) = app().await;
    // OAuth app registration (public, form-encoded like the Mastodon clients).
    let form = |uri: &str, body: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let resp = app
        .clone()
        .oneshot(form(
            "/api/v1/apps",
            "client_name=suite-teste&redirect_uris=urn:ietf:wg:oauth:2.0:oob",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());
    // A token with an unknown client never issues a credential.
    let resp = app
        .oneshot(form(
            "/oauth/token",
            "grant_type=client_credentials&client_id=nope&client_secret=nope",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "got {}", resp.status());
}

// ---------------------------------------------------------------------------
// FUNCTIONAL — social graph CRUD (mutes, blocks, filters, lists)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mute_and_block_roundtrip() {
    let (app, st) = app().await;
    let (org, _, cookie) = seed_session(&st.db).await;
    // A target in the SAME org, with a public profile (mute/block resolve the actor URL).
    let other = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at)
         VALUES ($1, $2, $3, 'email', now())",
    )
    .bind(other)
    .bind(org)
    .bind(format!("sub-{}", other.simple()))
    .execute(&st.db)
    .await
    .expect("seed other citizen");
    make_public(&st.db, other).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/accounts/{other}/mute"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "mute got {}", resp.status());
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/mutes", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/accounts/{other}/unmute"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/accounts/{other}/block"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/blocks", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/accounts/{other}/unblock"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn filters_crud_roundtrip() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/filters",
            Some(&cookie),
            r#"{"phrase":"frase-filtrada-teste","context":["home"]}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());
    let created = body_json(resp).await;
    let id = created["id"]
        .as_str()
        .or_else(|| created["data"]["id"].as_str())
        .expect("filter id")
        .to_owned();
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/filters", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/filters/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn lists_crud_roundtrip() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/lists",
            Some(&cookie),
            r#"{"title":"Minha lista de teste"}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());
    let created = body_json(resp).await;
    let id = created["id"]
        .as_str()
        .or_else(|| created["data"]["id"].as_str())
        .expect("list id")
        .to_owned();
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            &format!("/api/v1/lists/{id}"),
            Some(&cookie),
            r#"{"title":"Lista renomeada"}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/lists", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/lists/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

// ---------------------------------------------------------------------------
// SECURITY + FUNCTIONAL — auth flows (register, login rate, reset, me)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_with_valid_cpf_starts_verification() {
    // Without SMTP in the test environment the service enters DEV mode (it logs the URL)
    // but the HTTP contract is the same: 202 + status verification_sent.
    //
    // Since 0.65.0 (migrations 0651/0652/0653), citizen signup requires
    // RESIDENCE (a UF + an IBGE municipality that exists and belongs to that UF) on top of
    // the full name and birth date — this test was stuck on the old payload (e-mail/
    // password/document only) and started returning 400. The municipality is seeded here:
    // `municipio_ibge` is a reference table populated by a script
    // (`scripts/seed-municipios-ibge.sql`), not by a migration, so a clean test
    // database has no rows at all — the test plants its own.
    let (app, st) = app().await;
    let (org, citizen, _) = seed_session(&st.db).await;
    sqlx::query(
        "INSERT INTO municipio_ibge (codigo_ibge, nome, uf) VALUES (3550308, 'São Paulo', 'SP')
         ON CONFLICT (codigo_ibge) DO NOTHING",
    )
    .execute(&st.db)
    .await
    .expect("seed municipio");
    let body = format!(
        r#"{{"org_id":"{org}","email":"novo-{}@example.org","password":"senha-forte-123","cpf":"52998224725","nome_completo":"Maria Aparecida Silva","nascimento":"1985-03-12","uf":"SP","municipio_ibge":3550308}}"#,
        citizen.simple()
    );
    let resp = app
        .oneshot(json_req("POST", "/api/v1/auth/register", None, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["status"], "verification_sent");
}

#[tokio::test]
async fn login_rate_limits_by_ip() {
    let (app, st) = app().await;
    let (org, _, _) = seed_session(&st.db).await;
    let body =
        format!(r#"{{"org_id":"{org}","email":"forca-bruta@example.org","password":"errada"}}"#);
    // A unique IP PER RUN: the audit table persists across executions and a
    // fixed IP would already be rate-limited on the suite's second round.
    let b = Uuid::now_v7();
    let b = b.as_bytes();
    let ip = format!("10.{}.{}.{}", b[13], b[14], b[15]);
    // 10 attempts (default) from the same IP; the 11th must see 429.
    for _ in 0..10 {
        let mut req = json_req("POST", "/api/v1/auth/login", None, &body);
        req.headers_mut()
            .insert("x-forwarded-for", ip.parse().unwrap());
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    let mut req = json_req("POST", "/api/v1/auth/login", None, &body);
    req.headers_mut()
        .insert("x-forwarded-for", ip.parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn password_reset_request_is_enumeration_resistant() {
    let (app, st) = app().await;
    let (org, _, _) = seed_session(&st.db).await;
    let body = format!(r#"{{"org_id":"{org}","email":"nao-existe@example.org"}}"#);
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/auth/password-reset/request",
            None,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_me_reflects_session_and_logout_is_idempotent() {
    let (app, st) = app().await;
    let (_, _, cookie) = seed_session(&st.db).await;
    // The own profile via cookie (the legacy /auth/me belongs to the OIDC/bearer era).
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Logging out with no cookie at all still returns 200 — a stale tab never sees an error.
    let resp = app
        .oneshot(json_req("POST", "/api/v1/auth/logout", None, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// SECURITY + FUNCTIONAL — the admin surface (issue #8, step 4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_read_surface_is_gated_and_serves() {
    // Every admin read obeys the SAME rule: anonymous 401, ordinary session
    // 403, admin never 401/403 nor 5xx. One loop covers the nine lists.
    let (app, st) = app().await;
    // admin_ext validates the binding in the fixed DEFAULT org — the test admin
    // must live in it (the other admin modules do not filter by org).
    let default_org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let (org, citizen, admin_cookie) = seed_session_in_org(&st.db, default_org).await;
    grant_admin(&st.db, org, citizen).await;
    let (_, _, plain_cookie) = seed_session(&st.db).await;
    for path in [
        "/api/v1/admin/stats",
        "/api/v1/admin/users",
        "/api/v1/admin/federation/peers",
        "/api/v1/admin/users-rich",
        "/api/v1/admin/reports",
        "/api/v1/admin/audit",
        "/api/v1/admin/webhooks",
        "/api/v1/admin/announcements",
        "/api/v1/admin/email-templates",
    ] {
        let resp = app.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "anon GET {path}");
        let resp = app
            .clone()
            .oneshot(get_with_cookie(path, &plain_cookie))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "non-admin GET {path}");
        let resp = app
            .clone()
            .oneshot(get_with_cookie(path, &admin_cookie))
            .await
            .unwrap();
        let s = resp.status();
        assert!(
            s != StatusCode::UNAUTHORIZED && s != StatusCode::FORBIDDEN && !s.is_server_error(),
            "admin GET {path} got {s}"
        );
    }
}

#[tokio::test]
async fn webhooks_crud_roundtrip() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/webhooks",
            Some(&cookie),
            r#"{"url":"https://hooks.example.org/dsoc","events":["report.created"]}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "create got {}", resp.status());
    let created = body_json(resp).await;
    let id = created["data"]["id"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .expect("webhook id")
        .to_owned();
    let resp = app
        .clone()
        .oneshot(json_req(
            "PATCH",
            &format!("/api/v1/admin/webhooks/{id}"),
            Some(&cookie),
            r#"{"enabled":false}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "patch got {}", resp.status());
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/webhooks",
            Some(&cookie),
            r#"{"url":"https://hooks.example.org/x","events":["evento-que-nao-existe"]}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "evento inválido aceito");
    let resp = app
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/admin/webhooks/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "delete got {}", resp.status());
}

#[tokio::test]
async fn announcements_lifecycle() {
    let (app, st) = app().await;
    let (org, citizen, admin_cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let (_, _, citizen_cookie) = seed_session(&st.db).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/announcements",
            Some(&admin_cookie),
            r#"{"body":"Aviso de manutenção da suíte de testes","publish_now":true}"#,
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "create got {}", resp.status());
    let created = body_json(resp).await;
    let id = created["data"]["id"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .expect("announcement id")
        .to_owned();

    // Once published it appears in the active list for any logged-in citizen.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            "/api/v1/announcements/active",
            &citizen_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The citizen dismisses; the admin unpublishes; the active list still returns 200 (empty).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/announcements/{id}/dismiss"),
            Some(&citizen_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "dismiss got {}", resp.status());
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/admin/announcements/{id}/unpublish"),
            Some(&admin_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "unpublish got {}",
        resp.status()
    );
}

#[tokio::test]
async fn moderation_account_actions_roundtrip() {
    // suspend → unsuspend → silence → unsilence on a target account; each action
    // is idempotent from the admin's point of view and never 5xx.
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let (_, target, _) = seed_session(&st.db).await;
    for action in ["suspend", "unsuspend", "silence", "unsilence"] {
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/v1/admin/accounts/{target}/{action}"),
                Some(&cookie),
                "{}",
            ))
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{action} got {}", resp.status());
    }
}

#[tokio::test]
async fn me_admin_status_reflects_role() {
    let (app, st) = app().await;
    let (org, citizen, admin_cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let (_, _, plain_cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/admin-status", &admin_cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["is_admin"], true);
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/admin-status", &plain_cookie))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["data"]["is_admin"], false);
}

#[tokio::test]
async fn invitation_preview_unknown_token_is_invalid_not_error() {
    // The preview is public and enumeration-neutral: an unknown token returns
    // 200 {valid:false} — never 500, never another invitation's data.
    let (app, _) = app().await;
    let resp = app
        .oneshot(get("/api/v1/invitations/token-inexistente/preview"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["valid"], false);
}

#[tokio::test]
async fn delivery_receipts_are_public_and_empty_for_unknown_proposal() {
    // The "digital registered mail" timeline is public by design; a proposal with no warnings
    // returns an empty list — never 500, never 401.
    let (app, _) = app().await;
    let resp = app
        .oneshot(get(&format!(
            "/api/v1/proposals/{}/delivery-receipts",
            Uuid::now_v7()
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn embed_placar_serves_selfcontained_widget() {
    // The press widget is public, self-contained and never 500s; a missing
    // mandate is a clean 404.
    let (app, st) = app().await;
    let (org, _, _) = seed_session(&st.db).await;
    let mandate = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at)
         VALUES ($1, $2, 'deputado_federal', 'Dep. Placar Teste', 'gab@example.leg.br', now())",
    )
    .bind(mandate)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed mandate");
    let resp = app
        .clone()
        .oneshot(get(&format!("/embed/placar/{mandate}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Dep. Placar Teste"));
    assert!(html.contains("silêncios registrados"));
    let resp = app
        .oneshot(get(&format!("/embed/placar/{}", Uuid::now_v7())))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Campaign donations/funding (0.31, migration 0523)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn campanha_requires_session() {
    let (app, _st) = app().await;
    let resp = app.oneshot(get("/api/v1/me/campanha")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn campanha_write_gated_to_politico() {
    let (app, st) = app().await;
    let (_, _citizen, cookie) = seed_session(&st.db).await;

    // Without a mandate binding: the read returns 200 with the flag off…
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/campanha", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["is_politico"], serde_json::json!(false));

    // …and ANY write is 403.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campanha/lancamentos",
            Some(&cookie),
            r#"{"kind":"entrada","descricao":"Doação — pessoa física","valor_centavos":25000,"occurred_on":"2026-07-15"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app
        .oneshot(json_req(
            "PUT",
            "/api/v1/me/campanha/config",
            Some(&cookie),
            r#"{"meta_centavos":5000000,"is_published":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn campanha_politico_roundtrip() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    let mandate = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at)
         VALUES ($1, $2, 'deputado_federal', 'Mandato Campanha', 'gab@example.leg.br', now())",
    )
    .bind(mandate)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed mandate");
    sqlx::query(
        "INSERT INTO mandate_identity_binding
             (id, mandate_id, citizen_id, verification_level, verified_at, created_at)
         VALUES ($1, $2, $3, 'directory', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(mandate)
    .bind(citizen)
    .execute(&st.db)
    .await
    .expect("seed binding");

    // An entry with a receipt (a donation) is stored and returns the id.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campanha/lancamentos",
            Some(&cookie),
            r#"{"kind":"entrada","descricao":"Doação — pessoa física","valor_centavos":25000,
                "occurred_on":"2026-07-15","receipt_ref":"RE-2026-0001","donor_name":"Maria S."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = body_json(resp).await["data"]["id"]
        .as_str()
        .expect("entry id")
        .to_owned();

    // An outflow with a receipt is 400 (receipt/donor only apply to entries).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campanha/lancamentos",
            Some(&cookie),
            r#"{"kind":"saida","descricao":"Material gráfico","valor_centavos":90000,
                "occurred_on":"2026-07-15","receipt_ref":"RE-X"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Config upsert.
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            "/api/v1/me/campanha/config",
            Some(&cookie),
            r#"{"meta_centavos":5000000,"crowdfunding_url":"https://financiamento.example/tse","is_published":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The overview reflects the entry + config.
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/campanha", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["is_politico"], serde_json::json!(true));
    assert_eq!(
        body["data"]["lancamentos"][0]["descricao"],
        serde_json::json!("Doação — pessoa física")
    );
    assert_eq!(
        body["data"]["config"]["is_published"],
        serde_json::json!(true)
    );

    // Revocation: it leaves the list; a second revocation is 404.
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/me/campanha/lancamentos/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/me/campanha/lancamentos/{id}"),
            Some(&cookie),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/campanha", &cookie))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["data"]["lancamentos"], serde_json::json!([]));
}

#[tokio::test]
async fn campanha_publica_only_when_published() {
    let (app, st) = app().await;
    // Org default fixa — find_public_by_handle resolve contra ela.
    let default_org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let (org, citizen, cookie) = seed_session_in_org(&st.db, default_org).await;
    let handle = format!("cand{}", &citizen.simple().to_string()[..8]);
    sqlx::query("UPDATE citizen SET handle = $2, is_public = TRUE, display_name = 'Cand. Teste' WHERE id = $1")
        .bind(citizen)
        .bind(&handle)
        .execute(&st.db)
        .await
        .expect("public handle");
    let mandate = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at)
         VALUES ($1, $2, 'vereador', 'Mandato Pub', 'gab2@example.leg.br', now())",
    )
    .bind(mandate)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed mandate");
    sqlx::query(
        "INSERT INTO mandate_identity_binding
             (id, mandate_id, citizen_id, verification_level, verified_at, created_at)
         VALUES ($1, $2, $3, 'directory', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(mandate)
    .bind(citizen)
    .execute(&st.db)
    .await
    .expect("seed binding");

    // Before publishing: 404 for anonymous callers (without leaking the unpublished config).
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/campanha/{handle}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Records a donation and publishes.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campanha/lancamentos",
            Some(&cookie),
            r#"{"kind":"entrada","descricao":"Doação — pessoa física","valor_centavos":10000,
                "occurred_on":"2026-07-15","receipt_ref":"RE-2026-0002"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            "/api/v1/me/campanha/config",
            Some(&cookie),
            r#"{"meta_centavos":100000,"is_published":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Published: the anonymous public page serves totals + entries.
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/campanha/{handle}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["data"]["total_entradas_centavos"],
        serde_json::json!(10000)
    );
    assert_eq!(body["data"]["doacoes_count"], serde_json::json!(1));
    assert_eq!(
        body["data"]["display_name"],
        serde_json::json!("Cand. Teste")
    );

    // Despublica → volta a 404.
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            "/api/v1/me/campanha/config",
            Some(&cookie),
            r#"{"meta_centavos":100000,"is_published":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(get(&format!("/api/v1/campanha/{handle}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Campaign groups (0.39.0 — Phase 2.3)
// ---------------------------------------------------------------------------

/// Create a mandate + binding (directory level) for the citizen — makes them a "politician".
async fn seed_mandate_binding(db: &Db, org: Uuid, citizen: Uuid) -> Uuid {
    let mandate = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, is_candidate, created_at) \
         VALUES ($1, $2, 'vereador', 'Vereador Teste', 'v@camara.test', false, $3)",
    )
    .bind(mandate)
    .bind(org)
    .bind(Utc::now())
    .execute(db)
    .await
    .expect("seed mandate");
    sqlx::query(
        "INSERT INTO mandate_identity_binding \
         (id, mandate_id, citizen_id, verification_level, verified_at, created_at) \
         VALUES ($1, $2, $3, 'directory', $4, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(mandate)
    .bind(citizen)
    .bind(Utc::now())
    .execute(db)
    .await
    .expect("seed binding");
    mandate
}

#[tokio::test]
async fn campaign_group_full_flow() {
    let (app, st) = app().await;
    let (org, politico, owner_cookie) = seed_session(&st.db).await;
    seed_mandate_binding(&st.db, org, politico).await;

    // 1) The politician creates the group.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group",
            Some(&owner_cookie),
            r#"{"name":"Campanha da Fulana","description":"Vem construir comigo."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let group_id = body["data"]["id"].as_str().unwrap().to_owned();

    // 2) The owner publishes an update.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group/posts",
            Some(&owner_cookie),
            r#"{"body":"Primeira atualização da campanha!"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 3) A voter (another account in the same org) joins the group.
    let (_, _, voter_cookie) = seed_session_in_org(&st.db, org).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/campaign-groups/{group_id}/join"),
            Some(&voter_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Joining is idempotent — a second POST does not duplicate.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/campaign-groups/{group_id}/join"),
            Some(&voter_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4) Public page: 1 member, 1 post, and the voter sees sou_membro=true.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/campaign-groups/{group_id}"),
            &voter_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["member_count"], serde_json::json!(1));
    assert_eq!(body["data"]["sou_membro"], serde_json::json!(true));
    assert_eq!(body["data"]["posts"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["data"]["name"],
        serde_json::json!("Campanha da Fulana")
    );

    // 5) O eleitor sai; contagem volta a zero.
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/campaign-groups/{group_id}/join"),
            Some(&voter_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(get(&format!("/api/v1/campaign-groups/{group_id}")))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["data"]["member_count"], serde_json::json!(0));
    assert_eq!(body["data"]["sou_membro"], serde_json::json!(false));
}

#[tokio::test]
async fn campaign_group_create_requires_politico() {
    let (app, st) = app().await;
    // An ordinary account, with no mandate binding.
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group",
            Some(&cookie),
            r#"{"name":"Grupo intruso"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn campaign_group_join_requires_auth() {
    let (app, _st) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/campaign-groups/{}/join", Uuid::now_v7()),
            None,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn campaign_group_poll_flow() {
    let (app, st) = app().await;
    let (org, politico, owner) = seed_session(&st.db).await;
    seed_mandate_binding(&st.db, org, politico).await;

    // The politician creates the group.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group",
            Some(&owner),
            r#"{"name":"Campanha X"}"#,
        ))
        .await
        .unwrap();
    let gid = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Opens a directed poll.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group/polls",
            Some(&owner),
            r#"{"question":"Priorizar saúde no orçamento?"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let pid = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A voter answers.
    let (_, _, voter) = seed_session_in_org(&st.db, org).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/campaign-groups/{gid}/polls/{pid}/respond"),
            Some(&voter),
            r#"{"answer":"concordo"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Public page (the voter's cookie): the aggregate counts 1 + my answer.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/campaign-groups/{gid}"),
            &voter,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let poll = &body["data"]["polls"][0];
    assert_eq!(poll["tally"]["concordo"].as_i64().unwrap(), 1);
    assert_eq!(poll["tally"]["total"].as_i64().unwrap(), 1);
    assert_eq!(poll["my_answer"].as_str().unwrap(), "concordo");

    // An ordinary citizen does not open a poll (no group → 403).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/campaign-group/polls",
            Some(&voter),
            r#"{"question":"intruso"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The owner closes it; a new answer is refused (409).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/campaign-group/polls/{pid}/close"),
            Some(&owner),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/campaign-groups/{gid}/polls/{pid}/respond"),
            Some(&voter),
            r#"{"answer":"discordo"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// Super-admin: edit/hide/delete content (0.40.0 — SOCRATES)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_can_edit_and_hide_mandate_but_non_admin_cannot() {
    let (app, st) = app().await;
    let (org, admin, admin_cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, admin).await;
    let mandate = seed_mandate_binding(&st.db, org, admin).await;

    // Editar: renomeia o mandato e troca o partido.
    let resp = app
        .clone()
        .oneshot(json_req(
            "PATCH",
            &format!("/api/v1/admin/mandates/{mandate}"),
            Some(&admin_cookie),
            r#"{"display_name":"Nome Corrigido","party":"PT"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (name, party): (String, Option<String>) =
        sqlx::query_as("SELECT display_name, party FROM mandate WHERE id = $1")
            .bind(mandate)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(name, "Nome Corrigido");
    assert_eq!(party.as_deref(), Some("PT"));

    // Ocultar: hidden_at passa a != NULL.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/admin/mandates/{mandate}/hide"),
            Some(&admin_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hidden: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT hidden_at FROM mandate WHERE id = $1")
            .bind(mandate)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(hidden.is_some(), "mandato deve ficar oculto");

    // Unhide with ?on=false.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/admin/mandates/{mandate}/hide?on=false"),
            Some(&admin_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hidden: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT hidden_at FROM mandate WHERE id = $1")
            .bind(mandate)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(hidden.is_none(), "reexibido");

    // A non-admin (ordinary account) cannot edit → 403.
    let (_, _, plain_cookie) = seed_session_in_org(&st.db, org).await;
    let resp = app
        .oneshot(json_req(
            "PATCH",
            &format!("/api/v1/admin/mandates/{mandate}"),
            Some(&plain_cookie),
            r#"{"display_name":"Hack"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_hard_delete_requires_force() {
    let (app, st) = app().await;
    let (org, admin, admin_cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, admin).await;
    let mandate = seed_mandate_binding(&st.db, org, admin).await;

    // Without ?force=true → 400 (guards against deleting by accident).
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/admin/mandates/{mandate}"),
            Some(&admin_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // With ?force=true → cascading delete (the mandate is cleared, only the binding).
    let resp = app
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/admin/mandates/{mandate}?force=true"),
            Some(&admin_cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM mandate WHERE id = $1")
        .bind(mandate)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(gone, 0, "mandato apagado");
}

// ---------------------------------------------------------------------------
// CONSULTAS PARTICIPATIVAS (Fase 3.3, migration 0531)
// ---------------------------------------------------------------------------

fn consulta_create_body() -> String {
    let opens = Utc::now().to_rfc3339();
    let closes = (Utc::now() + Duration::days(7)).to_rfc3339();
    format!(
        r#"{{"title":"Prioridades 2027","opens_at":"{opens}","closes_at":"{closes}",
             "questions":["Transporte deve ser gratuito?","Mais creches?"]}}"#
    )
}

#[tokio::test]
async fn consultas_anonymous_cannot_create_or_respond() {
    let (app, _) = app().await;
    // Creating without a session → 401.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/consultas",
            None,
            &consulta_create_body(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Answering without a session → 401.
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/consultas/{}/responder", Uuid::now_v7()),
            None,
            r#"{"answers":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn consultas_plain_citizen_cannot_create() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    // An ordinary citizen (no admin, no mandate) → 403.
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/consultas",
            Some(&cookie),
            &consulta_create_body(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn consultas_full_participation_flow() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;

    // 1. The admin creates the consultation.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/consultas",
            Some(&cookie),
            &consulta_create_body(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp).await;
    let cid = created["data"]["id"].as_str().unwrap().to_owned();

    // 2. PUBLIC read (no cookie): 2 questions, aggregates at zero.
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/consultas/{cid}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = body_json(resp).await;
    let questions = detail["data"]["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0]["tally"]["total"].as_i64().unwrap(), 0);
    assert!(questions[0]["my_answer"].is_null());
    let q0 = questions[0]["id"].as_str().unwrap().to_owned();

    // 3. A logged-in citizen answers the first question.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/consultas/{cid}/responder"),
            Some(&cookie),
            &format!(r#"{{"answers":[{{"question_id":"{q0}","answer":"concordo"}}]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. Detalhe credenciado: minha resposta aparece + agregado conta 1.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/consultas/{cid}"),
            &cookie,
        ))
        .await
        .unwrap();
    let detail = body_json(resp).await;
    let q0v = &detail["data"]["questions"][0];
    assert_eq!(q0v["my_answer"].as_str().unwrap(), "concordo");
    assert_eq!(q0v["tally"]["concordo"].as_i64().unwrap(), 1);
    assert_eq!(q0v["tally"]["total"].as_i64().unwrap(), 1);

    // 5. Re-submission (upsert): switches to disagree, the total stays 1.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/consultas/{cid}/responder"),
            Some(&cookie),
            &format!(r#"{{"answers":[{{"question_id":"{q0}","answer":"discordo"}}]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/consultas/{cid}"),
            &cookie,
        ))
        .await
        .unwrap();
    let detail = body_json(resp).await;
    let q0v = &detail["data"]["questions"][0];
    assert_eq!(q0v["my_answer"].as_str().unwrap(), "discordo");
    assert_eq!(q0v["tally"]["discordo"].as_i64().unwrap(), 1);
    assert_eq!(q0v["tally"]["concordo"].as_i64().unwrap(), 0);
    assert_eq!(q0v["tally"]["total"].as_i64().unwrap(), 1);

    // 6. Closed: new answers are refused (409).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/consultas/{cid}/close"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/consultas/{cid}/responder"),
            Some(&cookie),
            &format!(r#"{{"answers":[{{"question_id":"{q0}","answer":"neutro"}}]}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// Office CRM (C6) — authorization gate
// ---------------------------------------------------------------------------

/// Seed a proposal directed at a mandate, with a public author, and the recipient
/// row (`proposal_target`) the CRM reads. Returns the proposal id.
async fn seed_directed_proposal(
    db: &Db,
    org: Uuid,
    mandate: Uuid,
    author: Uuid,
    title: &str,
) -> Uuid {
    let proposal = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proposal \
         (id, org_id, mandate_id, title, body, threshold, author_citizen_id, status, created_at) \
         VALUES ($1, $2, $3, $4, 'corpo da demanda', 10, $5, 'published', now())",
    )
    .bind(proposal)
    .bind(org)
    .bind(mandate)
    .bind(title)
    .bind(author)
    .execute(db)
    .await
    .expect("seed proposal");
    sqlx::query(
        "INSERT INTO proposal_target (proposal_id, mandate_id, created_at) \
         VALUES ($1, $2, now())",
    )
    .bind(proposal)
    .bind(mandate)
    .execute(db)
    .await
    .expect("seed proposal_target");
    proposal
}

/// SECURITY — the CRM is scoped to the logged-in operator's mandate: they only see
/// THEIR OWN CRM. An operator of office A never sees office B's demands; a
/// citizen with no binding gets 403; anonymous gets 401.
#[tokio::test]
async fn mandate_crm_scoped_to_operator_only() {
    let (app, st) = app().await;

    // Office A: an operator with a binding.
    let (org, operator_a, cookie_a) = seed_session(&st.db).await;
    let mandate_a = seed_mandate_binding(&st.db, org, operator_a).await;

    // Office B: another mandate in the same org, unrelated to operator A.
    let mandate_b = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at) \
         VALUES ($1, $2, 'vereador', 'Vereador B', 'b@camara.test', now())",
    )
    .bind(mandate_b)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed mandate B");

    // A public citizen author who directs proposals at both offices.
    let (_, author, _) = seed_session(&st.db).await;
    sqlx::query(
        "UPDATE citizen SET handle = 'fulana', display_name = 'Fulana', is_public = true \
         WHERE id = $1",
    )
    .bind(author)
    .execute(&st.db)
    .await
    .expect("author profile");
    let prop_a =
        seed_directed_proposal(&st.db, org, mandate_a, author, "Falta médico no posto").await;
    let prop_b = seed_directed_proposal(&st.db, org, mandate_b, author, "Buraco na rua do B").await;

    // Operator A sees THEIR CRM: exactly A's demand, never B's.
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/mandate/crm", &cookie_a))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["mandate_id"], mandate_a.to_string());
    assert_eq!(json["data"]["totals"]["contacts"], 1);
    assert_eq!(json["data"]["totals"]["demands"], 1);
    let demands = json["data"]["contacts"][0]["demands"].as_array().unwrap();
    assert_eq!(demands.len(), 1);
    assert_eq!(demands[0]["proposal_id"], prop_a.to_string());
    assert_ne!(demands[0]["proposal_id"], prop_b.to_string());
    // The author's public handle appears (already-public data); no e-mail/PII.
    assert_eq!(json["data"]["contacts"][0]["handle"], "fulana");
    assert!(json["data"]["contacts"][0].get("email").is_none());

    // A citizen with no mandate binding → 403 (not an operator of any office).
    let (_, _, plain_cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/mandate/crm", &plain_cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Anonymous (no cookie) → 401.
    let resp = app.oneshot(get("/api/v1/me/mandate/crm")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// SECURITY + FUNCTIONAL — mandato coletivo: compromisso consultivo (D8.1, 0666)
// ---------------------------------------------------------------------------

/// The write gate is the mandate binding: anonymous → 401, ordinary citizen → 403.
/// The public read of commitments is open (200) and leaks no private data.
#[tokio::test]
async fn commitments_write_is_gated_read_is_public() {
    let (app, st) = app().await;
    let (org, operator, op_cookie) = seed_session(&st.db).await;
    let mandate = seed_mandate_binding(&st.db, org, operator).await;

    // An anonymous caller does not create a commitment.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/commitments",
            None,
            r#"{"theme":"Tema","description":"Descrição do compromisso"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A citizen with no mandate binding → 403, and nothing is written.
    let (_, _, plain_cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/commitments",
            Some(&plain_cookie),
            r#"{"theme":"Intruso","description":"Não deveria gravar"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mandate_commitment WHERE theme = 'Intruso'")
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(count, 0);

    // The public read is open (the mandate still has no commitments).
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/politicos/{mandate}/commitments")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(
        json["data"]["commitments"].as_array().map(Vec::len),
        Some(0)
    );

    // The operator creates a valid commitment and tries an invalid outcome → 400.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/commitments",
            Some(&op_cookie),
            r#"{"theme":"Plano Diretor","description":"Vou ouvir a base antes de votar."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/commitments/{id}/outcome"),
            Some(&op_cookie),
            r#"{"outcome":"vinculante"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Full flow: the operator declares → opens a linked consultation → records that they followed it;
/// the public surface reflects the topic, the consultative kind, the outcome and the aggregate.
#[tokio::test]
async fn commitment_declare_consult_and_outcome_flow() {
    let (app, st) = app().await;
    // The consultation is created through ConsultationService, which scopes by org — the
    // operator must live in the fixed default org of the public surfaces.
    let default_org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let (org, operator, cookie) = seed_session_in_org(&st.db, default_org).await;
    let mandate = seed_mandate_binding(&st.db, org, operator).await;

    // 1) Declara o compromisso.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/commitments",
            Some(&cookie),
            r#"{"theme":"Reforma tributária","description":"Consultar a base antes de votar."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let id = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 2) Abre a consulta ligada (reusa o crate consultations).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/commitments/{id}/consult"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let consultation_id = body_json(resp).await["data"]["consultation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The consultation really was created in the consultations crate.
    let cc: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE id = $1")
            .bind(Uuid::parse_str(&consultation_id).unwrap())
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(cc, 1);

    // Opening it again is a conflict (the commitment already has a consultation).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/commitments/{id}/consult"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // 3) Records that they followed it, with a note.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/commitments/{id}/outcome"),
            Some(&cookie),
            r#"{"outcome":"seguiu","note":"Votei conforme a maioria da base."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4) The public surface reflects everything (no login).
    let resp = app
        .oneshot(get(&format!("/api/v1/politicos/{mandate}/commitments")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let list = json["data"]["commitments"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    let c = &list[0];
    assert_eq!(c["theme"], "Reforma tributária");
    assert_eq!(c["kind"], "consultivo");
    assert_eq!(c["outcome"], "seguiu");
    assert_eq!(c["outcome_note"], "Votei conforme a maioria da base.");
    assert_eq!(c["consultation"]["consultation_id"], consultation_id);
    assert_eq!(c["consultation"]["total"], 0);
}

// ---------------------------------------------------------------------------
// Participatory budgeting — mandate pilot (D8.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn op_anonymous_cannot_create_round() {
    let (app, _st) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/op/rounds",
            None,
            r#"{"title":"Emenda 2026","budget_cents":50000000}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn op_plain_citizen_cannot_create_round() {
    let (app, st) = app().await;
    // An ordinary account, no mandate binding → not an operator.
    let (_, _, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/op/rounds",
            Some(&cookie),
            r#"{"title":"Emenda 2026","budget_cents":50000000}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn op_full_cycle_operator_and_citizen() {
    let (app, st) = app().await;
    // The operator (with a mandate binding) opens the round.
    let (org, operator, op_cookie) = seed_session(&st.db).await;
    let mandate = seed_mandate_binding(&st.db, org, operator).await;

    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/op/rounds",
            Some(&op_cookie),
            r#"{"title":"Verba de emenda 2026","budget_cents":250,"uf":"SP"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let round_id = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A logged-in citizen (another account in the same org) submits an item in the proposals phase.
    let (_, _, voter_cookie) = seed_session_in_org(&st.db, org).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/op/rounds/{round_id}/items"),
            Some(&voter_cookie),
            r#"{"title":"Praça revitalizada","estimated_cents":150}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let item_id = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Voting before the voting phase → 409 (wrong phase).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/op/rounds/{round_id}/vote"),
            Some(&voter_cookie),
            &format!(r#"{{"item_id":"{item_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // The operator advances to 'votacao'.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/op/rounds/{round_id}/phase"),
            Some(&op_cookie),
            r#"{"phase":"votacao"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // An anonymous caller does not vote → 401.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/op/rounds/{round_id}/vote"),
            None,
            &format!(r#"{{"item_id":"{item_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // The citizen votes — and votes AGAIN (upsert): still 1 vote per round.
    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/v1/op/rounds/{round_id}/vote"),
                Some(&voter_cookie),
                &format!(r#"{{"item_id":"{item_id}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Public surface: 1 vote in total (the upsert did not duplicate), the item is ranked and fits.
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/op/rounds/{round_id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["total_votes"], 1);
    assert_eq!(json["data"]["mandate_id"], mandate.to_string());
    let items = json["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["votes"], 1);
    assert_eq!(items[0]["rank"], 1);
    assert_eq!(items[0]["fits_budget"], true);
    assert_eq!(json["data"]["allocated_cents"], 150);

    // The operator closes it and reports back (marks execution).
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/op/rounds/{round_id}/items/{item_id}/execution"),
            Some(&op_cookie),
            r#"{"execution_status":"concluido"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The round appears in the mandate's public listing.
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/politicos/{mandate}/op")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let rounds = json["data"]["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0]["id"], round_id);
    assert_eq!(rounds[0]["total_votes"], 1);
}

#[tokio::test]
async fn op_operator_cannot_touch_other_mandate_round() {
    let (app, st) = app().await;
    // Office A creates a round.
    let (org, op_a, cookie_a) = seed_session(&st.db).await;
    seed_mandate_binding(&st.db, org, op_a).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/me/mandate/op/rounds",
            Some(&cookie_a),
            r#"{"title":"Rodada do A","budget_cents":1000}"#,
        ))
        .await
        .unwrap();
    let round_a = body_json(resp).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // An operator of ANOTHER office cannot advance the phase of A's round → 404.
    let (_, op_b, cookie_b) = seed_session_in_org(&st.db, org).await;
    seed_mandate_binding(&st.db, org, op_b).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/me/mandate/op/rounds/{round_a}/phase"),
            Some(&cookie_b),
            r#"{"phase":"votacao"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// SOCRATES — mirror of e-Cidadania Legislative Ideas (migration 0670)
// ---------------------------------------------------------------------------
// SECURITY: both endpoints are owner/admin gated (anonymous → 401, ordinary
// citizen → 403). FUNCTIONAL: dedup by `ideia_id` → 409 `already_mirrored` with
// the existing topic in `data` — checked BEFORE the fetch, so the test NEVER
// touches the network towards the Senate.

#[tokio::test]
async fn anonymous_cannot_mirror_socrates_idea() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/mirror",
            None,
            r#"{"url_or_id":"165188"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_cannot_mirror_socrates_idea() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/mirror",
            Some(&cookie),
            r#"{"url_or_id":"165188"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn anonymous_cannot_list_socrates_mirrors() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(get("/api/v1/admin/socrates/mirrors"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_cannot_list_socrates_mirrors() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/admin/socrates/mirrors", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_mirror_rejects_invalid_input() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/mirror",
            Some(&cookie),
            r#"{"url_or_id":"não é id nem URL"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "invalid_input");
}

/// Seed an existing mirror (forum + topic + a socrates_mirror row) and
/// return `(ideia_id, topic_id)`. The topic's author is the test's own citizen —
/// the FK only requires a valid `citizen`.
async fn seed_socrates_mirror(db: &Db, org: Uuid, author: Uuid) -> (String, Uuid) {
    let now = Utc::now();
    let forum = Uuid::now_v7();
    let slug = format!("sen-teste-{}", &forum.simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, created_at)
         VALUES ($1, $2, $3, $3, 'Senado (teste)', 'institucional', $4)",
    )
    .bind(forum)
    .bind(org)
    .bind(&slug)
    .bind(now)
    .execute(db)
    .await
    .expect("seed forum");
    let topic = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
         VALUES ($1, $2, $3, 'Ideia espelhada (teste)', 'corpo', $4)",
    )
    .bind(topic)
    .bind(forum)
    .bind(author)
    .bind(now)
    .execute(db)
    .await
    .expect("seed topic");
    // A numeric ideia_id unique per execution (dedup is a global UNIQUE).
    let ideia_id = format!("9{:011}", topic.as_u128() % 100_000_000_000);
    sqlx::query(
        "INSERT INTO socrates_mirror (id, ideia_id, source_url, topic_id, created_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(&ideia_id)
    .bind(format!(
        "https://www12.senado.leg.br/ecidadania/visualizacaoideia?id={ideia_id}"
    ))
    .bind(topic)
    .bind(now)
    .execute(db)
    .await
    .expect("seed mirror");
    (ideia_id, topic)
}

/// Dedup: an already-mirrored idea answers 409 `already_mirrored` with the topic
/// existing one in `data` — and the check comes BEFORE the fetch (no network call).
#[tokio::test]
async fn admin_mirror_dedups_with_409_and_existing_topic() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let (ideia_id, topic_id) = seed_socrates_mirror(&st.db, org, citizen).await;

    // Id puro.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/mirror",
            Some(&cookie),
            &format!(r#"{{"url_or_id":"{ideia_id}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "already_mirrored");
    assert_eq!(v["data"]["topic_id"], topic_id.to_string());

    // The full URL of the same idea dedups identically.
    let url = format!("https://www12.senado.leg.br/ecidadania/visualizacaoideia?id={ideia_id}");
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/mirror",
            Some(&cookie),
            &format!(r#"{{"url_or_id":"{url}"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

/// The admin listing includes the seeded mirror, with the topic's title and path.
#[tokio::test]
async fn admin_lists_socrates_mirrors() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let (ideia_id, topic_id) = seed_socrates_mirror(&st.db, org, citizen).await;

    let resp = app
        .oneshot(get_with_cookie("/api/v1/admin/socrates/mirrors", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let list = v["data"].as_array().expect("lista");
    let entry = list
        .iter()
        .find(|e| e["ideia_id"] == ideia_id.as_str())
        .expect("espelho semeado na lista");
    assert_eq!(entry["topic_id"], topic_id.to_string());
    assert_eq!(entry["path"], format!("/f/topico/{topic_id}"));
    assert_eq!(entry["topic_title"], "Ideia espelhada (teste)");
    // 0671: a mirror with no sweep comes out as 'manual', still without a support counter.
    assert_eq!(entry["origin"], "manual");
    assert!(entry["apoiamentos"].is_null());
    assert!(entry["apoios_updated_at"].is_null());
    // 0672: a pre-v3 mirror arrives with the idea's fields empty and `body_synced_at`
    // null — that null is what the panel uses to offer the backfill.
    assert!(entry["apoiamentos_num"].is_null());
    assert!(entry["situacao"].is_null());
    assert!(entry["body_synced_at"].is_null());
}

// ---------------------------------------------------------------------------
// SOCRATES v2 — automatic sweep (migration 0671)
// ---------------------------------------------------------------------------
// SECURITY: both new endpoints carry the SAME owner/admin gate (anonymous →
// 401, ordinary citizen → 403). No test calls the Senate portal: the gate
// blocks before the sweep, and the round listing only reads the local log.

#[tokio::test]
async fn anonymous_cannot_run_socrates_sweep() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req("POST", "/api/v1/admin/socrates/sweep", None, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_cannot_run_socrates_sweep() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/sweep",
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn anonymous_cannot_list_socrates_runs() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(get("/api/v1/admin/socrates/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_cannot_list_socrates_runs() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(get_with_cookie("/api/v1/admin/socrates/runs", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// SOCRATES v3 — backfill of old mirrors (migration 0672)
// ---------------------------------------------------------------------------
// SECURITY: the backfill rewrites the body of ALL mirrored topics, so
// the owner/admin gate is what stops an ordinary citizen from firing N calls to
// the Senate portal and N writes to the forum. The gate blocks BEFORE any fetch
// — no test here touches the Senate.

#[tokio::test]
async fn anonymous_cannot_backfill_socrates_mirrors() {
    let (app, _) = app().await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/backfill",
            None,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn non_admin_cannot_backfill_socrates_mirrors() {
    let (app, st) = app().await;
    let (_org, _citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/socrates/backfill",
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "forbidden");
}

/// Seed a CLOSED round in the log and return its id — the admin reads the same shape
/// the worker writes, without any real round having to run.
async fn seed_socrates_run(db: &Db) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO socrates_sweep_run
             (id, started_at, finished_at, found, mirrored, skipped, error)
         VALUES ($1, $2, $3, 5, 2, 3, NULL)",
    )
    .bind(id)
    .bind(now)
    .bind(now)
    .execute(db)
    .await
    .expect("seed sweep run");
    id
}

/// The round listing returns the log with the round's counts.
#[tokio::test]
async fn admin_lists_socrates_sweep_runs() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    let run_id = seed_socrates_run(&st.db).await;

    let resp = app
        .oneshot(get_with_cookie("/api/v1/admin/socrates/runs", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let list = v["data"].as_array().expect("lista de rodadas");
    let entry = list
        .iter()
        .find(|e| e["id"] == run_id.to_string())
        .expect("rodada semeada na lista");
    assert_eq!(entry["found"], 5);
    assert_eq!(entry["mirrored"], 2);
    assert_eq!(entry["skipped"], 3);
    assert!(entry["error"].is_null());
    assert!(!entry["finished_at"].is_null());
}

// ---------------------------------------------------------------------------
// AGORA — creating a directory requires a responsible citizen (party_administrator is born with it)
// ---------------------------------------------------------------------------

/// Without a responsible citizen in the body → 400 and no directory created; with one
/// (by citizen_id) → 201 and the admin binding is born in the same transaction.
#[tokio::test]
async fn admin_create_directory_requires_and_binds_responsavel() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    // The route is permission-gated (R0.3): a role with `directory.manage` + a binding.
    let role_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO user_role (id, org_id, name, position, permissions)
         VALUES ($1, $2, 'Dirigente', 10, ARRAY['directory.manage','party.manage'])",
    )
    .bind(role_id)
    .bind(org)
    .execute(&st.db)
    .await
    .expect("seed role");
    sqlx::query(
        "INSERT INTO citizen_role_binding (id, org_id, citizen_id, role_id, created_at)
         VALUES ($1, $2, $3, $4, now())",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(citizen)
    .bind(role_id)
    .execute(&st.db)
    .await
    .expect("bind role");
    sqlx::query("INSERT INTO party (org_id, sigla, name) VALUES ($1, 'PT', 'PT')")
        .bind(org)
        .execute(&st.db)
        .await
        .expect("seed party");
    let responsavel = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at)
         VALUES ($1, $2, 'directory', $3)",
    )
    .bind(responsavel)
    .bind(org)
    .bind(Utc::now())
    .execute(&st.db)
    .await
    .expect("seed responsável");

    // Without a responsible citizen → 400 missing_responsavel, nothing created.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/parties/PT/directories",
            Some(&cookie),
            r#"{"esfera":"estadual","uf":"RS","name":"Diretório sem dono"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert_eq!(v["error"]["code"], "missing_responsavel");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM party_directory WHERE org_id = $1 AND name = 'Diretório sem dono'",
    )
    .bind(org)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(count, 0);

    // With a responsible citizen → 201 + an admin binding scoped to the directory.
    let resp = app
        .oneshot(json_req(
            "POST",
            "/api/v1/admin/parties/PT/directories",
            Some(&cookie),
            &format!(
                r#"{{"esfera":"estadual","uf":"RS","name":"Diretório Estadual — RS","responsavel_citizen_id":"{responsavel}"}}"#
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_json(resp).await;
    let dir_id = Uuid::parse_str(v["data"].as_str().expect("id do diretório")).unwrap();
    let (bound_citizen, bound_role): (Uuid, String) = sqlx::query_as(
        "SELECT citizen_id, role FROM party_administrator
         WHERE org_id = $1 AND party_sigla = 'PT' AND directory_id = $2",
    )
    .bind(org)
    .bind(dir_id)
    .fetch_one(&st.db)
    .await
    .expect("responsável vinculado na criação");
    assert_eq!(bound_citizen, responsavel);
    assert_eq!(bound_role, "admin");
}

// ---------------------------------------------------------------------------
// BRANDING — runtime visual identity (migration 0674)
// ---------------------------------------------------------------------------

/// Full lifecycle of the admin-editable branding: public defaults, auth gates,
/// validated upsert, and the public read reflecting the stored state.
#[tokio::test]
async fn branding_public_defaults_gates_and_roundtrip() {
    let (app, st) = app().await;

    // 1. Never configured: the public endpoint degrades to empty defaults.
    let resp = app.clone().oneshot(get("/api/v1/branding")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["data"]["site_name"].is_null());
    assert_eq!(body["data"]["colors"], serde_json::json!({}));

    // 2. Anonymous PUT is rejected outright.
    let resp = app
        .clone()
        .oneshot(json_req("PUT", "/api/v1/admin/branding", None, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 3. A logged-in NON-admin is forbidden.
    let (org, citizen, cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            "/api/v1/admin/branding",
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 4. Admin upserts a valid identity.
    grant_admin(&st.db, org, citizen).await;
    let valid = r##"{"site_name":"Pindorama","tagline":"Democracia com consequência",
        "logo_url":"/media/logo.png",
        "colors":{"accent":"#22c55e","accent-strong":"#115c2d"}}"##;
    let resp = app
        .clone()
        .oneshot(json_req(
            "PUT",
            "/api/v1/admin/branding",
            Some(&cookie),
            valid,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["site_name"], "Pindorama");
    assert_eq!(body["data"]["colors"]["accent"], "#22c55e");

    // 5. Validation walls: unknown token, non-hex value, unsafe URL.
    for bad in [
        r##"{"colors":{"background":"#000"}}"##,
        r##"{"colors":{"accent":"url(javascript:alert(1))"}}"##,
        r##"{"logo_url":"javascript:alert(1)"}"##,
    ] {
        let resp = app
            .clone()
            .oneshot(json_req(
                "PUT",
                "/api/v1/admin/branding",
                Some(&cookie),
                bad,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "payload should be rejected: {bad}"
        );
    }

    // 6. The public read (same org, via session) reflects the stored state,
    //    and the admin panel load returns the same payload.
    for uri in ["/api/v1/branding", "/api/v1/admin/branding"] {
        let resp = app
            .clone()
            .oneshot(get_with_cookie(uri, &cookie))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET {uri}");
        let body = body_json(resp).await;
        assert_eq!(body["data"]["site_name"], "Pindorama", "GET {uri}");
        assert_eq!(body["data"]["colors"]["accent-strong"], "#115c2d");
    }
}

// ---------------------------------------------------------------------------
// TAG-A-REPRESENTATIVE — citizens mark a mandate on a cause (0676, issue #3)
// ---------------------------------------------------------------------------

/// Seed forum + visible topic; returns the topic id.
async fn seed_topic(db: &Db, org: Uuid, author: Uuid) -> Uuid {
    let now = Utc::now();
    let forum = Uuid::now_v7();
    let slug = format!("rep-teste-{}", &forum.simple().to_string()[..12]);
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, created_at)
         VALUES ($1, $2, $3, $3, 'Fórum (teste)', 'institucional', $4)",
    )
    .bind(forum)
    .bind(org)
    .bind(&slug)
    .bind(now)
    .execute(db)
    .await
    .expect("seed forum");
    let topic = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
         VALUES ($1, $2, $3, 'Isenção teste (causa)', 'corpo', $4)",
    )
    .bind(topic)
    .bind(forum)
    .bind(author)
    .bind(now)
    .execute(db)
    .await
    .expect("seed topic");
    topic
}

/// Seed a mandate with a public e-mail; returns the mandate id.
async fn seed_rep_mandate(db: &Db, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, is_candidate, \
         created_at, party, uf, sphere) \
         VALUES ($1, $2, 'deputado_federal', $3, $4, false, $5, 'PT', 'SP', 'federal')",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(format!("gab-{}@camara.test", id.simple()))
    .bind(Utc::now())
    .execute(db)
    .await
    .expect("seed mandate");
    id
}

#[tokio::test]
async fn representative_tag_full_lifecycle_and_privacy() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    let topic = seed_topic(&st.db, org, citizen).await;
    let mandate = seed_rep_mandate(&st.db, org, "Dep. Teste").await;

    // Anonymous cannot tag.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            None,
            &format!("{{\"mandate_id\":\"{mandate}\"}}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // The citizen tags their representative.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            Some(&cookie),
            &format!("{{\"mandate_id\":\"{mandate}\"}}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A second pick ADDS (0677: multiple representatives per citizen, capped).
    let mandate2 = seed_rep_mandate(&st.db, org, "Dep. Dois").await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            Some(&cookie),
            &format!("{{\"mandate_id\":\"{mandate2}\"}}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Public aggregate: BOTH picks count, `mine` lists both — and the raw
    // payload NEVER contains the citizen's UUID (LGPD posture).
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/topics/{topic}/representatives"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["total_tags"], 2, "body={body}");
    let mine: Vec<String> = body["data"]["mine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(mine.contains(&mandate.to_string()) && mine.contains(&mandate2.to_string()));
    assert!(
        !body.to_string().contains(&citizen.to_string()),
        "citizen UUID leaked in aggregate payload"
    );

    // Duplicate pick is idempotent; the cap (5) refuses the 6th DISTINCT one.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            Some(&cookie),
            &format!("{{\"mandate_id\":\"{mandate2}\"}}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "duplicate is a no-op");
    for i in 0..3 {
        let extra = seed_rep_mandate(&st.db, org, &format!("Dep. Extra {i}")).await;
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/v1/topics/{topic}/representatives"),
                Some(&cookie),
                &format!("{{\"mandate_id\":\"{extra}\"}}"),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let sixth = seed_rep_mandate(&st.db, org, "Dep. Sexto").await;
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            Some(&cookie),
            &format!("{{\"mandate_id\":\"{sixth}\"}}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "cap of 5");

    // Unknown mandate is a validation error; unknown topic a 404.
    let resp = app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("/api/v1/topics/{topic}/representatives"),
            Some(&cookie),
            &format!("{{\"mandate_id\":\"{}\"}}", Uuid::now_v7()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let resp = app
        .clone()
        .oneshot(get(&format!(
            "/api/v1/topics/{}/representatives",
            Uuid::now_v7()
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Untag ONE pick: the rest stay (total drops by exactly one).
    let resp = app
        .clone()
        .oneshot(json_req(
            "DELETE",
            &format!("/api/v1/topics/{topic}/representatives/{mandate2}"),
            Some(&cookie),
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            &format!("/api/v1/topics/{topic}/representatives"),
            &cookie,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["data"]["total_tags"], 4,
        "5 picks minus the removed one"
    );
    assert!(!body["data"]["mine"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some(&mandate2.to_string())));
}

#[tokio::test]
async fn representative_daily_sweep_is_idempotent_and_consolidated() {
    let (_, st) = app().await;
    let (org, citizen, _) = seed_session(&st.db).await;
    let topic = seed_topic(&st.db, org, citizen).await;
    let mandate = seed_rep_mandate(&st.db, org, "Dep. Sweep").await;

    // Two citizens tagged YESTERDAY (the sweep's window).
    //
    // Anchored to the sweep's own definition of "yesterday" — the CALENDAR day —
    // and placed at midday inside it. The previous `Utc::now() - 26h` only lands on
    // that day when the current UTC hour is >= 02:00: run between 00:00 and 02:00
    // UTC it fell a day earlier than the sweep looks, and the test failed. That is
    // a two-hour window of red every single day, and it caught us live at 00:04 UTC.
    let yesterday = (Utc::now() - Duration::days(1))
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .expect("midday is a valid time")
        .and_utc();
    for _ in 0..2 {
        let other = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
             VALUES ($1, $2, $3, 'email', $4)",
        )
        .bind(other)
        .bind(org)
        .bind(format!("sub-{}", other.simple()))
        .bind(Utc::now())
        .execute(&st.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO topic_representative_tag \
             (org_id, topic_id, mandate_id, citizen_id, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(org)
        .bind(topic)
        .bind(mandate)
        .bind(other)
        .bind(yesterday)
        .execute(&st.db)
        .await
        .unwrap();
    }

    // First sweep: claims (mandate, day) with the consolidated count.
    // No SMTP in tests -> DEV mode (logged), still marked sent.
    dsoc_gateway::topic_representatives::daily_alert_sweep(&st.db, "https://test.example").await;
    let (count, sent): (i32, bool) = sqlx::query_as(
        "SELECT tag_count, sent_at IS NOT NULL FROM mandate_alert_delivery \
         WHERE mandate_id = $1",
    )
    .bind(mandate)
    .fetch_one(&st.db)
    .await
    .expect("delivery row");
    assert_eq!(count, 2, "consolidated: ONE row for TWO tags");
    assert!(sent, "DEV mode still marks the daily send as satisfied");

    // Second sweep: idempotent — still exactly one delivery row.
    dsoc_gateway::topic_representatives::daily_alert_sweep(&st.db, "https://test.example").await;
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mandate_alert_delivery WHERE mandate_id = $1")
            .bind(mandate)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(rows, 1, "sweep must never double-send a day");
}

// ---------------------------------------------------------------------------
// SECURITY — Group/forum inbox requires a verified HTTP Signature (issue #6)
// ---------------------------------------------------------------------------

/// Seed a federated forum reachable at the handle it returns.
async fn seed_federated_forum(db: &Db, org: Uuid) -> (Uuid, String) {
    let forum = Uuid::now_v7();
    // UUIDv7 is time-ordered: the LEADING hex is shared by uuids minted in the
    // same millisecond, so slice the TRAILING (random) half for the slug.
    let hex = forum.simple().to_string();
    let slug = format!("sec{}", &hex[hex.len() - 12..]);
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, federated, created_at)
         VALUES ($1, $2, $3, $3, 'Fórum federado (teste)', 'institucional', true, $4)",
    )
    .bind(forum)
    .bind(org)
    .bind(&slug)
    .bind(Utc::now())
    .execute(db)
    .await
    .expect("seed federated forum");
    (forum, slug)
}

/// An UNSIGNED activity must never mutate forum state. Before issue #6 the
/// forum branch ran before the signature section, so this `Undo{Follow}`
/// deleted the follower row outright.
#[tokio::test]
async fn unsigned_group_inbox_activity_is_rejected_and_mutates_nothing() {
    let (app, st) = app().await;
    let org = uuid::uuid!("11111111-1111-1111-1111-111111111111");
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org', $3)
                 ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", org.simple()))
    .bind(Utc::now())
    .execute(&st.db)
    .await
    .expect("seed org");
    let (forum, slug) = seed_federated_forum(&st.db, org).await;

    // A follower the attacker will try to remove without any signature.
    let victim = "https://remote.example/users/victim";
    sqlx::query(
        "INSERT INTO forum_follower (forum_id, remote_actor_url, remote_inbox_url, accepted_at)
         VALUES ($1, $2, 'https://remote.example/inbox', now())",
    )
    .bind(forum)
    .bind(victim)
    .execute(&st.db)
    .await
    .expect("seed follower");

    let undo = serde_json::json!({
        "id": "https://evil.example/activities/undo-1",
        "type": "Undo",
        "actor": victim,
        "object": { "type": "Follow", "actor": victim }
    })
    .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::CONTENT_TYPE, "application/activity+json")
                .body(Body::from(undo))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "unsigned Group activity must be refused"
    );

    let still_there: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM forum_follower WHERE forum_id = $1 AND remote_actor_url = $2)",
    )
    .bind(forum)
    .bind(victim)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert!(
        still_there,
        "SECURITY REGRESSION: unsigned Undo{{Follow}} deleted a forum_follower row"
    );
}

/// A malformed/garbage Signature header is refused before any forum lookup
/// side effect — and an unknown handle still 404s (no enumeration change).
#[tokio::test]
async fn group_inbox_rejects_malformed_signature_and_unknown_handle() {
    let (app, st) = app().await;
    let org = uuid::uuid!("11111111-1111-1111-1111-111111111111");
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org', $3)
                 ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", org.simple()))
    .bind(Utc::now())
    .execute(&st.db)
    .await
    .expect("seed org");
    let (_, slug) = seed_federated_forum(&st.db, org).await;

    let body = serde_json::json!({
        "id": "https://evil.example/activities/x",
        "type": "Follow",
        "actor": "https://evil.example/users/x"
    })
    .to_string();

    // Garbage Signature header -> 400 (parse), never a mutation.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header("signature", "not-a-signature")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::UNAUTHORIZED,
        "got {}",
        resp.status()
    );

    // Unknown handle stays 404 even unsigned.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/actors/nao-existe-mesmo/inbox")
                .header(header::HOST, "democracia.social.br")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// SECURITY — the respond link expires, is single-use and is revocable (issue #12)
//
// `POST /respond` is UNAUTHENTICATED and writes a mandate's public official
// response. The token used to be `hmac(secret, sla_id)`: deterministic, eternal,
// replayable, and revocable only by rotating the global secret — which killed
// every link at once. Possession of a stale URL was standing authority to speak
// in an official's name.
// ---------------------------------------------------------------------------

/// Seeds an SLA with a live link and returns `(sla_id, token)`.
async fn seed_respond_link(db: &Db, expires_in_days: i64, revoked: bool) -> (Uuid, String) {
    let sla = Uuid::now_v7();
    let token: String = (0..64)
        .map(|i| char::from(b'a' + u8::try_from(i % 6).unwrap()))
        .collect();
    let hash = {
        use sha2::Digest as _;
        sha2::Sha256::digest(token.as_bytes()).to_vec()
    };
    sqlx::query(
        "INSERT INTO respond_link (id, sla_id, token_hash, expires_at, revoked_at)
         VALUES ($1, $2, $3, now() + make_interval(days => $4::int), $5)",
    )
    .bind(Uuid::now_v7())
    .bind(sla)
    .bind(hash)
    .bind(i32::try_from(expires_in_days).unwrap())
    .bind(revoked.then(Utc::now))
    .execute(db)
    .await
    .expect("seed respond_link");
    (sla, token)
}

fn respond_context(sla: Uuid, token: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/v1/respond/context?sla={sla}&t={token}"))
        .body(Body::empty())
        .unwrap()
}

/// A live link is accepted; an EXPIRED one is refused with 410, not 403 — the
/// official needs to know the link died rather than that they are unauthorised.
#[tokio::test]
async fn respond_link_expires() {
    let (app, st) = app().await;

    let (sla_live, token_live) = seed_respond_link(&st.db, 30, false).await;
    let resp = app
        .clone()
        .oneshot(respond_context(sla_live, &token_live))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a live link must not be refused as invalid"
    );
    assert_ne!(resp.status(), StatusCode::GONE, "a live link is not spent");

    let (sla_dead, token_dead) = seed_respond_link(&st.db, -1, false).await;
    let resp = app
        .oneshot(respond_context(sla_dead, &token_dead))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "SECURITY REGRESSION: an expired respond link was still honoured"
    );
}

/// A revoked link is refused even though it has not expired and was never used.
#[tokio::test]
async fn respond_link_is_individually_revocable() {
    let (app, st) = app().await;
    let (sla, token) = seed_respond_link(&st.db, 30, true).await;
    let resp = app.oneshot(respond_context(sla, &token)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "SECURITY REGRESSION: a revoked respond link was still honoured"
    );
}

/// THE REPLAY: a link already spent must not authorise a second response. This is
/// what a captured URL buys an attacker — the previous token bought unlimited use.
#[tokio::test]
async fn respond_link_cannot_be_replayed_after_use() {
    let (app, st) = app().await;
    let (sla, token) = seed_respond_link(&st.db, 30, false).await;

    // Mark it spent, exactly as a recorded response does.
    sqlx::query("UPDATE respond_link SET used_at = now() WHERE sla_id = $1")
        .bind(sla)
        .execute(&st.db)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(respond_context(sla, &token))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "SECURITY REGRESSION: a spent respond link was replayable"
    );

    // And the write path refuses too, not only the read path.
    let body = serde_json::json!({
        "sla_id": sla.to_string(),
        "token": token,
        "body": "resposta replayed",
        "committed": true,
    })
    .to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/respond")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "SECURITY REGRESSION: a spent link could still POST an official response"
    );
}

/// A token belonging to another SLA never authorises this one, and guessing is
/// bounded by the attempt counter.
#[tokio::test]
async fn respond_link_rejects_a_foreign_token_and_counts_attempts() {
    let (app, st) = app().await;
    let (sla_a, _) = seed_respond_link(&st.db, 30, false).await;
    let foreign = "f".repeat(64);

    let resp = app
        .clone()
        .oneshot(respond_context(sla_a, &foreign))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM respond_link WHERE sla_id = $1")
        .bind(sla_a)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert!(
        attempts >= 1,
        "a failed presentation must be counted, so guessing is bounded"
    );
}

// ---------------------------------------------------------------------------
// SECURITY — the SSRF guard on every server-side fetch (issue #9)
//
// The federation inbox takes an actor URL straight out of an UNAUTHENTICATED
// `Signature` header. Before the guard, `starts_with("https://")` was the only
// thing between that string and a request into the pod's network.
//
// These drive the real handler and the real client. The address tables live in
// unit tests next to the code; what is proven here is that the surfaces are
// actually WIRED to the guard.
// ---------------------------------------------------------------------------

use dsoc_gateway::outbound::{guarded_get, OutboundError, OutboundPolicy};

/// Every internal destination is refused, whether written as a literal address or
/// reached through a public-looking NAME that resolves inward.
#[tokio::test]
async fn guarded_fetch_refuses_internal_destinations() {
    let p = OutboundPolicy::default();
    for url in [
        "https://127.0.0.1/actor",
        "https://[::1]/actor",
        "https://10.0.0.1/actor",
        "https://192.168.1.1/actor",
        "https://169.254.169.254/latest/meta-data/", // cloud metadata
        "https://localhost/actor",                   // a NAME resolving to loopback
    ] {
        let err = guarded_get(url, &[], &p).await.unwrap_err();
        assert!(
            matches!(err, OutboundError::BlockedAddress(_)),
            "{url} must be refused as a blocked address, got {err:?}"
        );
    }
}

/// Plain HTTP is refused unless a surface opts in, so a downgrade cannot be used to
/// strip TLS from a fetch the platform makes on someone's behalf.
#[tokio::test]
async fn guarded_fetch_refuses_plain_http_by_default() {
    let p = OutboundPolicy::default();
    let err = guarded_get("http://example.com/actor", &[], &p)
        .await
        .unwrap_err();
    assert_eq!(err, OutboundError::SchemeNotAllowed("http".to_owned()));
}

// NOT covered by an automated test, and deliberately said out loud:
//
// * the BODY CAP — every address a local test server can bind is one the address
//   guard refuses first, so an integration test would pass through the address
//   check while claiming to test the cap. That is exactly what the first version of
//   this file did. The cap is pinned instead by unit tests over synthetic chunks in
//   `outbound.rs`, including a peer that lies in `Content-Length`.
// * REDIRECT REFUSAL — `reqwest::redirect::Policy::none()` is set when the client is
//   built and reqwest exposes no way to read the policy back, so this rests on
//   construction and review rather than on a test.

/// The inbox path itself: a `keyId` pointing at an internal address must not make the
/// gateway dial it. The signature is bogus, so a refusal is expected either way — what
/// this pins is that the failure is NOT a successful internal fetch.
#[tokio::test]
async fn inbox_key_id_cannot_reach_an_internal_address() {
    let (app, st) = app().await;
    let (_, slug, body) = inbox_fixture(&st).await;

    let digest = digest_header_for(&body);
    let date = http_date_now();
    let sig = "keyId=\"https://169.254.169.254/latest/meta-data/#main-key\",\
               algorithm=\"rsa-sha256\",headers=\"(request-target) host date digest\",\
               signature=\"AAAA\"";

    let started = std::time::Instant::now();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::DATE, date)
                .header("digest", digest)
                .header("signature", sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        !resp.status().is_success(),
        "an internal keyId must never produce a successful inbox delivery"
    );
    // The status alone proves nothing: the request fails either way in a sandbox
    // with no metadata service, so this assertion held even with the guard REMOVED.
    // What distinguishes a guarded fetch is that NO DIAL HAPPENS — refusal is
    // immediate, where an unguarded attempt sits on the 10s connect timeout.
    // Measured on the removal experiment: 10.1s unguarded vs 4.0s guarded.
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "the guard must refuse before dialling; took {elapsed:?}, which means a \
         connection to the internal address was attempted"
    );
}

// ---------------------------------------------------------------------------
// SECURITY — the module gate fails CLOSED on a database error (issue #18)
//
// `flag_enabled` used to collapse "no row" and "DB error" into one `None` via
// `.ok().flatten()`, and `None` means "use the manifest default" — ON for 20 of
// the 26 modules. So a database blip silently RE-ENABLED a module an admin had
// switched off, and the answer was cached, outliving the blip by 30s.
//
// The unit tests in module_gate.rs pin the decision table. These two drive a
// REAL failing query through the real code path, because the bug lived in the
// error handling, not in the decision.
// ---------------------------------------------------------------------------

/// A state whose pool points nowhere: any query returns Err, which is precisely
/// the condition the old code mistook for "no row".
fn state_with_unreachable_db(good: &dsoc_app::AppState) -> dsoc_app::AppState {
    let mut broken = good.clone();
    broken.db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(250))
        .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/nonexistent")
        .expect("lazy pool");
    broken
}

#[tokio::test]
async fn module_gate_denies_when_the_database_is_unreachable() {
    let (_, st) = app().await;
    let broken = state_with_unreachable_db(&st);
    // A fresh org so the process-global flag cache cannot answer for us.
    let org = Uuid::now_v7();

    // `forums` is non-core with `default_enabled: true` — exactly the shape that
    // used to be switched back ON by a failing lookup.
    let verdict = dsoc_gateway::module_gate::require_module(&broken, org, "forums").await;
    assert!(
        verdict.is_err(),
        "SECURITY REGRESSION: a DB error re-enabled a module (fail-open)"
    );
}

#[tokio::test]
async fn module_gate_does_not_cache_a_failed_lookup() {
    let (_, st) = app().await;
    let org = Uuid::now_v7();

    // First, fail.
    let broken = state_with_unreachable_db(&st);
    assert!(
        dsoc_gateway::module_gate::require_module(&broken, org, "forums")
            .await
            .is_err()
    );

    // Then recover immediately on a healthy pool. If the error had been cached,
    // this would stay denied for the 30s TTL — a blip becoming an outage.
    let verdict = dsoc_gateway::module_gate::require_module(&st, org, "forums").await;
    assert!(
        verdict.is_ok(),
        "a failed lookup must not be cached — recovery has to be immediate"
    );
}

// ---------------------------------------------------------------------------
// TENANT SCOPE — the federation tables now carry an org (issue #14, phase 1)
//
// These four tables shipped with NO org column, so isolation could not be
// enforced even in principle: there was nothing to filter on. Migration 0681
// added `org_id NOT NULL`, which is the FOUNDATION — Row-Level Security is
// explicitly not part of this phase (see the migration header for why).
//
// What these pin is that the column is populated CORRECTLY and cannot be
// skipped, because a NOT NULL that every INSERT satisfies with the wrong value
// buys nothing.
// ---------------------------------------------------------------------------

/// A follow row inherits the org of the citizen that owns it — derived, not passed,
/// so it cannot disagree with its owner.
#[tokio::test]
async fn a_follow_inherits_the_org_of_its_owner() {
    let (_, st) = app().await;
    let org_b = Uuid::now_v7();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org B', $3)")
        .bind(org_b)
        .bind(format!("org-{}", org_b.simple()))
        .bind(Utc::now())
        .execute(&st.db)
        .await
        .expect("seed org B");
    let (_, citizen_b, _) = seed_session_in_org(&st.db, org_b).await;

    let remote = format!("https://remote.example/users/u{}", Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO federation_follow
             (id, org_id, citizen_id, direction, remote_actor_url, remote_inbox_url, created_at)
         VALUES ($1, (SELECT org_id FROM citizen WHERE id = $2), $2, 'outbound', $3, $4, now())",
    )
    .bind(Uuid::now_v7())
    .bind(citizen_b)
    .bind(&remote)
    .bind("https://remote.example/inbox")
    .execute(&st.db)
    .await
    .expect("insert follow");

    let stored: Uuid =
        sqlx::query_scalar("SELECT org_id FROM federation_follow WHERE remote_actor_url = $1")
            .bind(&remote)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(stored, org_b, "the follow must belong to its owner's org");

    // And a query scoped to the OTHER org must not see it — the property the
    // column exists to make expressible at all.
    let visible_in_a: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM federation_follow WHERE remote_actor_url = $1 AND org_id = $2",
    )
    .bind(&remote)
    .bind(uuid::uuid!("11111111-1111-1111-1111-111111111111"))
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(visible_in_a, 0, "org A must not see org B's follow");
}

/// The NOT NULL is load-bearing: an INSERT that forgets the org is refused by the
/// database, so a row can never become unattributable.
#[tokio::test]
async fn an_insert_without_an_org_is_refused_by_the_database() {
    let (_, st) = app().await;
    let (_, citizen, _) = seed_session(&st.db).await;

    for sql in [
        "INSERT INTO federation_follow (id, citizen_id, direction, remote_actor_url, created_at)
         VALUES ($1, $2, 'outbound', 'https://x.example/u', now())",
        "INSERT INTO federation_outbox_entry
             (id, citizen_id, activity_id, kind, visibility, payload, created_at)
         VALUES ($1, $2, 'https://x.example/a', 'Create', 'public', '{}'::jsonb, now())",
    ] {
        let err = sqlx::query(sql)
            .bind(Uuid::now_v7())
            .bind(citizen)
            .execute(&st.db)
            .await;
        assert!(
            err.is_err(),
            "an INSERT without org_id must be refused, not silently accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// SECURITY — admin authority stops at the org boundary (issue #8)
//
// The gate used to be `EXISTS(... WHERE citizen_id=$1 ...)` with no org filter,
// copied into sixteen modules. An admin of ANY org therefore passed the admin
// gate of EVERY org, and the multi-tenant model was a naming convention.
//
// The scenario below is that exact escalation: a citizen whose SESSION is in
// org A, holding an admin binding only in org B. Before the fix the org-less
// EXISTS found the org-B binding and let them in.
// ---------------------------------------------------------------------------

/// Every admin read must refuse a caller whose admin binding is in ANOTHER org.
#[tokio::test]
async fn admin_of_another_org_is_refused_across_the_admin_surface() {
    let (app, st) = app().await;
    let org_a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let org_b = Uuid::now_v7();

    // Session in org A…
    let (_, citizen, cookie) = seed_session_in_org(&st.db, org_a).await;
    // …but the admin binding lives in org B.
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org B', $3)")
        .bind(org_b)
        .bind(format!("org-{}", org_b.simple()))
        .bind(Utc::now())
        .execute(&st.db)
        .await
        .expect("seed org B");
    grant_admin(&st.db, org_b, citizen).await;

    for path in [
        "/api/v1/admin/users",
        "/api/v1/admin/users-rich",
        "/api/v1/admin/reports",
        "/api/v1/admin/webhooks",
        "/api/v1/admin/announcements",
        "/api/v1/admin/email-templates",
    ] {
        let resp = app
            .clone()
            .oneshot(get_with_cookie(path, &cookie))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "SECURITY REGRESSION: an admin of another org reached {path}"
        );
    }
}

/// The same caller, granted in their OWN org, passes — proving the test above
/// measures the org boundary and not merely a broken admin surface.
#[tokio::test]
async fn admin_of_the_callers_own_org_still_passes() {
    let (app, st) = app().await;
    let org_a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let (_, citizen, cookie) = seed_session_in_org(&st.db, org_a).await;
    grant_admin(&st.db, org_a, citizen).await;

    for path in ["/api/v1/admin/users", "/api/v1/admin/webhooks"] {
        let resp = app
            .clone()
            .oneshot(get_with_cookie(path, &cookie))
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "an admin of their own org must pass {path}, got {}",
            resp.status()
        );
    }
}

/// A platform-role grant may not reach a citizen of another org. Before the fix
/// the target org came from the request BODY, so an admin of org A wrote an
/// `admin_role_binding` row in org B — self-promotion across the boundary.
#[tokio::test]
async fn platform_role_grant_cannot_cross_the_org_boundary() {
    let (app, st) = app().await;
    let org_a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let org_b = Uuid::now_v7();
    let (_, admin, cookie) = seed_session_in_org(&st.db, org_a).await;
    grant_admin(&st.db, org_a, admin).await;

    // A victim living entirely in org B.
    let (_, victim, _) = seed_session_in_org(&st.db, org_b).await;

    // The body still carries an `org_id` — a client may send anything. It must be
    // ignored, and the grant refused because the TARGET is not in the caller's org.
    let body = serde_json::json!({ "role": "owner", "org_id": org_b.to_string() }).to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/admin/users/{victim}/platform-role"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    // NOT `assert_ne!(OK)`: a wrong method or path would satisfy that vacuously —
    // which is exactly how the first version of this test passed while measuring
    // nothing. 404 is the intended answer: the target does not exist HERE.
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "SECURITY REGRESSION: a cross-org platform-role grant was not refused"
    );

    let leaked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admin_role_binding WHERE citizen_id = $1 AND org_id = $2)",
    )
    .bind(victim)
    .bind(org_b)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert!(
        !leaked,
        "SECURITY REGRESSION: an admin_role_binding row was written in another org"
    );
}

/// `whoami` must report the role the caller holds IN THEIR OWN ORG. Reporting a
/// role held elsewhere makes the front end render admin controls the API refuses.
#[tokio::test]
async fn whoami_does_not_report_a_role_held_in_another_org() {
    let (app, st) = app().await;
    let org_a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let org_b = Uuid::now_v7();
    let (_, citizen, cookie) = seed_session_in_org(&st.db, org_a).await;
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org B', $3)")
        .bind(org_b)
        .bind(format!("org-{}", org_b.simple()))
        .bind(Utc::now())
        .execute(&st.db)
        .await
        .expect("seed org B");
    grant_admin(&st.db, org_b, citizen).await;

    let resp = app
        .oneshot(get_with_cookie("/api/v1/me/whoami", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(
        v["data"]["is_admin"], true,
        "SECURITY REGRESSION: whoami reported admin from a binding in another org"
    );
}

// ---------------------------------------------------------------------------
// Retention of the inbound-activity idempotency logs (issue #10).
//
// These ran as a background loop that only LOGS its errors, so the first
// version shipped to production broken (42883: `make_interval` has no `bigint`
// overload) and stayed silent for a full release. The loop body is split out
// precisely so a test can execute the real DELETE against real PostgreSQL —
// a unit test with a mocked pool would have reproduced the bug, not caught it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inbox_seen_retention_deletes_only_rows_past_the_window() {
    let (_, st) = app().await;
    let org = uuid::uuid!("11111111-1111-1111-1111-111111111111");
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org', $3)
                 ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", org.simple()))
    .bind(Utc::now())
    .execute(&st.db)
    .await
    .expect("seed org");
    let (forum, _) = seed_federated_forum(&st.db, org).await;

    let old_id = format!("https://remote.example/activities/old-{}", Uuid::now_v7());
    let new_id = format!("https://remote.example/activities/new-{}", Uuid::now_v7());
    for (activity_id, age_days) in [(&old_id, 90_i64), (&new_id, 1_i64)] {
        sqlx::query(
            "INSERT INTO forum_inbox_seen (activity_id, forum_id, seen_at)
             VALUES ($1, $2, now() - make_interval(days => $3::int))",
        )
        .bind(activity_id)
        .bind(forum)
        .bind(i32::try_from(age_days).unwrap())
        .execute(&st.db)
        .await
        .expect("seed inbox_seen row");
    }

    // The call under test — this is the statement that was failing in production.
    let pruned = dsoc_gateway::worker::prune_inbox_seen(&st.db, "forum_inbox_seen", 30)
        .await
        .expect("retention DELETE must succeed against real PostgreSQL");
    assert!(pruned >= 1, "the 90-day-old row must be pruned");

    let old_gone: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM forum_inbox_seen WHERE activity_id = $1)",
    )
    .bind(&old_id)
    .fetch_one(&st.db)
    .await
    .unwrap();
    let new_kept: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM forum_inbox_seen WHERE activity_id = $1)")
            .bind(&new_id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(old_gone, "a row past the window must be deleted");
    assert!(
        new_kept,
        "a row inside the window must survive — pruning it would let a live redelivery replay"
    );
}

/// Every table the loop sweeps must accept the statement. Catches the case where
/// one log has a different column type or name from the other.
#[tokio::test]
async fn inbox_seen_retention_runs_against_every_configured_table() {
    let (_, st) = app().await;
    for table in dsoc_gateway::worker::INBOX_SEEN_TABLES {
        dsoc_gateway::worker::prune_inbox_seen(&st.db, table, 30)
            .await
            .unwrap_or_else(|e| panic!("retention DELETE failed on {table}: {e}"));
    }
}

// ---------------------------------------------------------------------------
// SECURITY — the inbound signature must be bound to THIS request (issue #10)
//
// #6 made the Group inbox require a signature. A signature covers HEADERS, not
// the body, so on its own it still allows a captured request to be replayed
// against another path, another host, or with a different body. These tests
// pin the four bindings that close that.
//
// Discriminator used throughout: a request that PASSES this gate goes on to
// fetch the signer's actor document, which fails in the test environment with
// 502 BAD_GATEWAY. So 401 here proves the rejection happened at the #10 gate,
// before any outbound call — and a 502 would prove the gate did NOT fire.
// ---------------------------------------------------------------------------

/// `Digest: SHA-256=<base64>` for a body, the header a real peer sends.
fn digest_header_for(body: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    format!(
        "SHA-256={}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body.as_bytes()))
    )
}

/// A syntactically valid `Signature` header covering `covered`. The signature
/// bytes are bogus on purpose: every test here must be refused BEFORE the
/// cryptographic check, so the bytes are never reached.
fn signature_header_covering(covered: &str) -> String {
    format!(
        "keyId=\"https://evil.example/users/x#main-key\",algorithm=\"rsa-sha256\",\
         headers=\"{covered}\",signature=\"AAAA\""
    )
}

fn http_date_now() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Seeds an org + federated forum and returns the pieces the cases below share.
async fn inbox_fixture(st: &dsoc_app::AppState) -> (Uuid, String, String) {
    let org = uuid::uuid!("11111111-1111-1111-1111-111111111111");
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Org', $3)
                 ON CONFLICT (id) DO NOTHING",
    )
    .bind(org)
    .bind(format!("org-{}", org.simple()))
    .bind(Utc::now())
    .execute(&st.db)
    .await
    .expect("seed org");
    let (forum, slug) = seed_federated_forum(&st.db, org).await;
    let body = serde_json::json!({
        "id": "https://evil.example/activities/hardening",
        "type": "Follow",
        "actor": "https://evil.example/users/x",
    })
    .to_string();
    (forum, slug, body)
}

/// THE ATTACK: a captured request, validly signed by its origin, whose BODY is
/// swapped before replay. The header signature still verifies — only the Digest
/// check stands in the way. `Delete` is used as the swapped body precisely
/// because it is destructive.
#[tokio::test]
async fn inbox_rejects_a_body_swapped_after_signing() {
    let (app, st) = app().await;
    let (forum, slug, signed_body) = inbox_fixture(&st).await;

    // A follower the swapped activity would try to destroy.
    let victim = "https://remote.example/users/victim-10";
    sqlx::query(
        "INSERT INTO forum_follower (forum_id, remote_actor_url, remote_inbox_url, accepted_at)
         VALUES ($1, $2, 'https://remote.example/inbox', now())",
    )
    .bind(forum)
    .bind(victim)
    .execute(&st.db)
    .await
    .expect("seed follower");

    let swapped_body = serde_json::json!({
        "id": "https://evil.example/activities/hardening",
        "type": "Undo",
        "actor": victim,
        "object": { "type": "Follow", "actor": victim },
    })
    .to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::DATE, http_date_now())
                // Digest of what was SIGNED; body is what was SENT.
                .header("digest", digest_header_for(&signed_body))
                .header(
                    "signature",
                    signature_header_covering("(request-target) host date digest"),
                )
                .body(Body::from(swapped_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a body swapped after signing must be refused at the digest gate"
    );

    let still_there: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM forum_follower WHERE forum_id = $1 AND remote_actor_url = $2)",
    )
    .bind(forum)
    .bind(victim)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert!(
        still_there,
        "SECURITY REGRESSION: a body-swapped Undo{{Follow}} deleted a forum_follower row"
    );
}

/// A missing `Digest` header is refused — otherwise the body is simply unbound.
#[tokio::test]
async fn inbox_rejects_a_missing_digest() {
    let (app, st) = app().await;
    let (_, slug, body) = inbox_fixture(&st).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::DATE, http_date_now())
                .header(
                    "signature",
                    signature_header_covering("(request-target) host date digest"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A `Date` outside the skew window is refused, so a capture stops being
/// replayable once the window passes.
#[tokio::test]
async fn inbox_rejects_a_stale_date() {
    let (app, st) = app().await;
    let (_, slug, body) = inbox_fixture(&st).await;

    let stale = (Utc::now() - chrono::Duration::hours(26))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::DATE, stale)
                .header("digest", digest_header_for(&body))
                .header(
                    "signature",
                    signature_header_covering("(request-target) host date digest"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Each required covered header is load-bearing: dropping any ONE of the four
/// must be refused, even when every actual header is present and correct.
#[tokio::test]
async fn inbox_rejects_signatures_that_undercover_the_request() {
    let (app, st) = app().await;
    let (_, slug, body) = inbox_fixture(&st).await;

    // Also covers the parser default: an absent `headers` parameter means
    // `["date"]`, which is the weakest possible coverage.
    let insufficient = [
        "host date digest",             // no (request-target) → any path
        "(request-target) date digest", // no host → any instance
        "(request-target) host digest", // no date → forever
        "(request-target) host date",   // no digest → any body
        "date",                         // the parser's bare default
    ];

    for covered in insufficient {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/actors/{slug}/inbox"))
                    .header(header::HOST, "democracia.social.br")
                    .header(header::DATE, http_date_now())
                    .header("digest", digest_header_for(&body))
                    .header("signature", signature_header_covering(covered))
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "coverage {covered:?} must be refused"
        );
    }
}

/// The gate must fire BEFORE the signer's actor document is fetched, so a
/// forged Signature header cannot aim this server's outbound HTTP at a host of
/// the attacker's choosing. Proof: full, correct coverage with a good digest
/// and a fresh date reaches the fetch and fails 502 — a DIFFERENT status from
/// the 401 the cases above produce. If this ever returns 401, the ordering
/// changed; if the cases above ever return 502, the gate stopped firing.
#[tokio::test]
async fn inbox_gate_runs_before_any_outbound_actor_fetch() {
    let (app, st) = app().await;
    let (_, slug, body) = inbox_fixture(&st).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/actors/{slug}/inbox"))
                .header(header::HOST, "democracia.social.br")
                .header(header::DATE, http_date_now())
                .header("digest", digest_header_for(&body))
                .header(
                    "signature",
                    signature_header_covering("(request-target) host date digest"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a fully-bound request must pass the #10 gate and reach the signature check"
    );
}
