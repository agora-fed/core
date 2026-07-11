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
/// resolve handles contra a org default fixa, então testes federados
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
    // Bot que preenche o campo escondido recebe 200 "ok" — sem SMTP, sem efeito.
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
    // 5/h por IP (default). O 6º do mesmo IP tem que ver 429 — antes de
    // qualquer tentativa de SMTP.
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
    // Sessão comum (sem mandato, sem partido) não pode atestar ninguém.
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
    // Anônimo: 401.
    let resp = app
        .clone()
        .oneshot(get("/api/v1/admin/email_domain_blocks"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Sessão comum: 403 — nunca a lista.
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
    // Admin bloqueia o domínio pela API real.
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
    // Cadastro com e-mail do domínio bloqueado: 403 opaco do gate.
    let register = format!(
        r#"{{"org_id":"{org}","email":"x@blocked-gate-test.example","password":"senha-forte-123","cpf":"00000000000"}}"#
    );
    let resp = app
        .clone()
        .oneshot(json_req("POST", "/api/v1/auth/register", None, &register))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // Removida a regra, o mesmo request volta a cair na validação normal
    // (CPF inválido = 4xx de validação, nunca o 403 do gate).
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
    // IP dentro do deny: 403 do gate, antes de qualquer verificação de credencial.
    let mut req = json_req("POST", "/api/v1/auth/login", None, &login);
    req.headers_mut()
        .insert("x-forwarded-for", "198.51.100.9".parse().unwrap());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // IP fora do deny: passa o gate e cai no 401 normal de credencial errada.
    // IP único por run — a auditoria de tentativas persiste entre execuções
    // e um IP fixo chegaria rate-limitado (429) na enésima rodada.
    let b = Uuid::now_v7();
    let b = b.as_bytes();
    let outside_ip = format!("10.{}.{}.{}", b[13], b[14], b[15]);
    let mut req = json_req("POST", "/api/v1/auth/login", None, &login);
    req.headers_mut()
        .insert("x-forwarded-for", outside_ip.parse().unwrap());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Cleanup: regra é global — remover pra não vazar pros outros testes.
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
    // Handle inexistente/inválido nunca devolve 200 nem 500 pra um peer AP —
    // a superfície responde 4xx (400 pra shape inválido, 404 pra ausente).
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
    // Atestador: operador de mandato (binding verificado).
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

    // Atesta com nota.
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

    // Lista pública mostra 1 + flags do viewer logado.
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

/// Torna o cidadão da sessão um perfil público federável (handle + is_public).
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
    // Fluxo federado completo: perfil público publica uma nota via a API
    // Mastodon-compat, e a superfície ActivityPub serve actor/outbox/followers.
    let (app, st) = app().await;
    // A resolução de handle da superfície federada usa a org default fixa.
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
    // Registro de app OAuth (público, form-encoded como os clientes Mastodon).
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
    // Token com client desconhecido nunca emite credencial.
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
    // Alvo na MESMA org, com perfil público (mute/block resolvem o actor URL).
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
    // Sem SMTP no ambiente de teste o serviço entra em DEV mode (loga a URL)
    // mas o contrato HTTP é o mesmo: 202 + status verification_sent.
    let (app, st) = app().await;
    let (org, citizen, _) = seed_session(&st.db).await;
    let body = format!(
        r#"{{"org_id":"{org}","email":"novo-{}@example.org","password":"senha-forte-123","cpf":"52998224725"}}"#,
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
    // IP único POR RUN: a tabela de auditoria persiste entre execuções e um
    // IP fixo chegaria já rate-limitado na segunda rodada da suíte.
    let b = Uuid::now_v7();
    let b = b.as_bytes();
    let ip = format!("10.{}.{}.{}", b[13], b[14], b[15]);
    // 10 tentativas (default) do mesmo IP; a 11ª tem que ver 429.
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
    // O perfil próprio via cookie (o /auth/me legado é da era OIDC/bearer).
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Logout sem cookie nenhum continua 200 — aba velha nunca vê erro.
    let resp = app
        .oneshot(json_req("POST", "/api/v1/auth/logout", None, "{}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// SECURITY + FUNCTIONAL — superfície admin (issue #8, passo 4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_read_surface_is_gated_and_serves() {
    // Toda leitura admin obedece a MESMA régua: anônimo 401, sessão comum
    // 403, admin nunca 401/403 nem 5xx. Um loop cobre as nove listas.
    let (app, st) = app().await;
    // admin_ext valida o binding na org DEFAULT fixa — o admin de teste
    // precisa viver nela (os demais módulos admin não filtram por org).
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

    // Publicado aparece na lista ativa de qualquer cidadão logado.
    let resp = app
        .clone()
        .oneshot(get_with_cookie(
            "/api/v1/announcements/active",
            &citizen_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Cidadão descarta; admin despublica; segue 200 na lista ativa (vazia).
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
    // suspend → unsuspend → silence → unsilence numa conta alvo; cada ação
    // é idempotente do ponto de vista do admin e nunca 5xx.
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
    // Preview é público e enumeração-neutro: token desconhecido devolve
    // 200 {valid:false} — nunca 500, nunca dados de outro convite.
    let (app, _) = app().await;
    let resp = app
        .oneshot(get("/api/v1/invitations/token-inexistente/preview"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["valid"], false);
}
