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
async fn anonymous_cannot_whoami() {
    let (app, _) = app().await;
    let resp = app.oneshot(get("/api/v1/me/whoami")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// `GET /me/whoami` (mobile): um cidadão comum logado retorna civic_type=cidadao,
/// sem papel de admin/partido e sem mandato. Verifica a composição consolidada.
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

/// SECURITY (2026-07-24 regressão): a identidade do caller vem dos headers
/// `x-dsoc-citizen-id`/`x-dsoc-org-id`/`x-citizen-id`, que o `inject_identity`
/// só pode setar a partir de uma sessão/bearer REAL. Um cliente que os injeta
/// direto (sem cookie) NÃO pode ser aceito — senão personifica qualquer
/// cidadão, inclusive admin. Antes do fix este request retornava 200 com os
/// stats de admin; agora os headers são apagados e cai em 401.
#[tokio::test]
async fn spoofed_identity_headers_are_stripped() {
    let (app, st) = app().await;
    let (org, citizen, _cookie) = seed_session(&st.db).await;
    grant_admin(&st.db, org, citizen).await;
    // Sem cookie, mas forjando os headers de identidade do admin recém-criado.
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
    //
    // O cadastro do cidadão exige, desde a 0.65.0 (migrations 0651/0652/0653),
    // o DOMICÍLIO (UF + município IBGE que exista e pertença à UF) além de nome
    // completo e nascimento — este teste ficou parado no payload antigo (só
    // e-mail/senha/CPF) e passou a bater 400. O município vem semeado aqui:
    // `municipio_ibge` é tabela de referência populada por script
    // (`scripts/seed-municipios-ibge.sql`), não por migration, então um banco de
    // teste limpo não tem nenhuma linha — o teste planta a sua.
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

#[tokio::test]
async fn delivery_receipts_are_public_and_empty_for_unknown_proposal() {
    // A timeline do "AR digital" é pública por design; proposta sem avisos
    // devolve lista vazia — nunca 500, nunca 401.
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
    // O widget da imprensa é público, autocontido e nunca 500; mandato
    // inexistente é 404 limpo.
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
// Doações/financiamento de campanha (0.31, migration 0523)
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

    // Sem vínculo de mandato: a leitura responde 200 com a flag desligada…
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/campanha", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["is_politico"], serde_json::json!(false));

    // …e QUALQUER escrita é 403.
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

    // Entrada com recibo (doação) grava e volta o id.
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

    // Saída com recibo é 400 (recibo/doador só valem em entrada).
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

    // Overview reflete lançamento + config.
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

    // Revogação: some da lista; segunda revogação é 404.
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

    // Antes de publicar: 404 anônimo (sem vazar a config despublicada).
    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/campanha/{handle}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Lança uma doação e publica.
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

    // Publicada: página pública anônima serve totais + lançamentos.
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
// Grupos de campanha (0.39.0 — Fase 2.3)
// ---------------------------------------------------------------------------

/// Cria um mandato + binding (nível directory) pro citizen — vira "político".
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

    // 1) Político cria o grupo.
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

    // 2) Dono publica uma atualização.
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

    // 3) Um eleitor (outra conta na mesma org) entra no grupo.
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

    // Join é idempotente — segundo POST não duplica.
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

    // 4) Página pública: 1 membro, 1 post, e o eleitor vê sou_membro=true.
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
    // Conta comum, sem binding de mandato.
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

    // Político cria o grupo.
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

    // Abre uma enquete dirigida.
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

    // Um eleitor responde.
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

    // Página pública (cookie do eleitor): agregado conta 1 + minha resposta.
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

    // Cidadão comum não abre enquete (sem grupo → 403).
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

    // Dono encerra; nova resposta é recusada (409).
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
// Super-admin: editar/ocultar/apagar conteúdo (0.40.0 — SOCRATES)
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

    // Reexibir com ?on=false.
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

    // Não-admin (conta comum) não edita → 403.
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

    // Sem ?force=true → 400 (protege contra apagar sem querer).
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

    // Com ?force=true → apaga em cascata (mandato limpo, só o binding).
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
    // Criar sem sessão → 401.
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
    // Responder sem sessão → 401.
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
    // Cidadão comum (sem admin, sem mandato) → 403.
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

    // 1. Admin cria a consulta.
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

    // 2. Leitura PÚBLICA (sem cookie): 2 perguntas, agregados zerados.
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

    // 3. Cidadão logado responde à primeira pergunta.
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

    // 5. Reenvio (upsert): muda para discordo, total continua 1.
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

    // 6. Encerrada: novas respostas são recusadas (409).
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
// CRM de gabinete (C6) — gate de autorização
// ---------------------------------------------------------------------------

/// Semeia uma proposta dirigida a um mandato, com autor público, e a linha de
/// destinatário (`proposal_target`) que o CRM lê. Devolve o id da proposta.
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

/// SECURITY — o CRM é escopado ao mandato do operador logado: só ele vê o CRM
/// DELE. Um operador do gabinete A jamais vê as demandas do gabinete B; um
/// cidadão sem vínculo recebe 403; anônimo recebe 401.
#[tokio::test]
async fn mandate_crm_scoped_to_operator_only() {
    let (app, st) = app().await;

    // Gabinete A: operador com vínculo.
    let (org, operator_a, cookie_a) = seed_session(&st.db).await;
    let mandate_a = seed_mandate_binding(&st.db, org, operator_a).await;

    // Gabinete B: outro mandato no mesmo org, sem relação com o operador A.
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

    // Autor cidadão público que dirige propostas aos dois gabinetes.
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

    // Operador A vê o CRM DELE: exatamente a demanda de A, nunca a de B.
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
    // O handle público do autor aparece (dado já público); nenhum e-mail/PII.
    assert_eq!(json["data"]["contacts"][0]["handle"], "fulana");
    assert!(json["data"]["contacts"][0].get("email").is_none());

    // Cidadão sem vínculo de mandato → 403 (não é operador de nenhum gabinete).
    let (_, _, plain_cookie) = seed_session(&st.db).await;
    let resp = app
        .clone()
        .oneshot(get_with_cookie("/api/v1/me/mandate/crm", &plain_cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Anônimo (sem cookie) → 401.
    let resp = app.oneshot(get("/api/v1/me/mandate/crm")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// SECURITY + FUNCTIONAL — mandato coletivo: compromisso consultivo (D8.1, 0666)
// ---------------------------------------------------------------------------

/// O gate de escrita é o vínculo de mandato: anônimo → 401, cidadão comum → 403.
/// A leitura pública dos compromissos é aberta (200) e não vaza dado privado.
#[tokio::test]
async fn commitments_write_is_gated_read_is_public() {
    let (app, st) = app().await;
    let (org, operator, op_cookie) = seed_session(&st.db).await;
    let mandate = seed_mandate_binding(&st.db, org, operator).await;

    // Anônimo não cria compromisso.
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

    // Cidadão sem vínculo de mandato → 403, e nada é gravado.
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

    // A leitura pública é aberta (mandato ainda sem compromissos).
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

    // O operador cria um compromisso válido e tenta um outcome inválido → 400.
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

/// Fluxo completo: operador declara → abre consulta ligada → registra que seguiu;
/// a superfície pública reflete tema, kind consultivo, outcome e o agregado.
#[tokio::test]
async fn commitment_declare_consult_and_outcome_flow() {
    let (app, st) = app().await;
    // A consulta é criada via ConsultationService, que escopa por org — o
    // operador precisa viver na org default fixa das superfícies públicas.
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
    // A consulta foi mesmo criada no crate consultations.
    let cc: i64 =
        sqlx::query_scalar("SELECT count(*) FROM consultations_consultation WHERE id = $1")
            .bind(Uuid::parse_str(&consultation_id).unwrap())
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(cc, 1);

    // Abrir de novo é conflito (compromisso já tem consulta).
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

    // 3) Registra que seguiu, com nota.
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

    // 4) A superfície pública reflete tudo (sem login).
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
// Orçamento participativo — piloto de mandato (D8.3)
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
    // Conta comum, sem binding de mandato → não é operador.
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
    // Operador (vínculo de mandato) abre a rodada.
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

    // Cidadão logado (outra conta na mesma org) submete um item na fase propostas.
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

    // Votar antes da fase de votação → 409 (fase errada).
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

    // Operador avança para 'votacao'.
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

    // Anônimo não vota → 401.
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

    // Cidadão vota — e vota DE NOVO (upsert): continua 1 voto por rodada.
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

    // Superfície pública: 1 voto total (upsert não duplicou), item ranqueado e cabe.
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

    // Operador fecha e presta contas (marca execução).
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

    // A rodada aparece na lista pública do mandato.
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
    // Gabinete A cria uma rodada.
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

    // Operador de OUTRO gabinete não consegue avançar a fase da rodada do A → 404.
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
// SOCRATES — espelho de Ideias Legislativas do e-Cidadania (migration 0670)
// ---------------------------------------------------------------------------
// SECURITY: os dois endpoints são gate owner/admin (anônimo → 401, cidadão
// comum → 403). FUNCTIONAL: dedup por `ideia_id` → 409 `already_mirrored` com
// o tópico existente no `data` — checado ANTES do fetch, então o teste NUNCA
// dispara rede pro Senado.

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

/// Semeia um espelho existente (fórum + tópico + linha em socrates_mirror) e
/// devolve `(ideia_id, topic_id)`. O autor do tópico é o próprio cidadão do
/// teste — a FK só exige um `citizen` válido.
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
    // ideia_id numérica única por execução (dedup é UNIQUE global).
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

/// Dedup: uma ideia já espelhada responde 409 `already_mirrored` com o tópico
/// existente no `data` — e o check vem ANTES do fetch (nenhuma chamada de rede).
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

    // A URL completa da mesma ideia deduplica igual.
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

/// A listagem admin inclui o espelho semeado, com título do tópico e caminho.
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
    // 0671: espelho sem sweep sai como 'manual', ainda sem contador de apoios.
    assert_eq!(entry["origin"], "manual");
    assert!(entry["apoiamentos"].is_null());
    assert!(entry["apoios_updated_at"].is_null());
    // 0672: espelho pré-v3 vem com os campos da ideia vazios e `body_synced_at`
    // nulo — é esse nulo que o painel usa pra oferecer o backfill.
    assert!(entry["apoiamentos_num"].is_null());
    assert!(entry["situacao"].is_null());
    assert!(entry["body_synced_at"].is_null());
}

// ---------------------------------------------------------------------------
// SOCRATES v2 — sweep automático (migration 0671)
// ---------------------------------------------------------------------------
// SECURITY: os dois endpoints novos têm o MESMO gate owner/admin (anônimo →
// 401, cidadão comum → 403). Nenhum teste chama o portal do Senado: o gate
// barra antes do sweep, e a listagem de rodadas só lê o log local.

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
// SOCRATES v3 — backfill dos espelhos antigos (migration 0672)
// ---------------------------------------------------------------------------
// SECURITY: o backfill reescreve o corpo de TODOS os tópicos espelhados, então
// o gate owner/admin é o que impede um cidadão comum de disparar N chamadas ao
// portal do Senado e N escritas no fórum. O gate barra ANTES de qualquer fetch
// — nenhum teste aqui toca o Senado.

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

/// Semeia uma rodada FECHADA no log e devolve o id — o admin lê o mesmo shape
/// que o worker grava, sem que nenhuma rodada real precise rodar.
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

/// A listagem de rodadas devolve o log com as contagens da rodada.
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
// ÁGORA — criação de diretório exige responsável (party_administrator nasce junto)
// ---------------------------------------------------------------------------

/// Sem responsável no corpo → 400 e nenhum diretório criado; com responsável
/// (por citizen_id) → 201 e o vínculo admin nasce na mesma transação.
#[tokio::test]
async fn admin_create_directory_requires_and_binds_responsavel() {
    let (app, st) = app().await;
    let (org, citizen, cookie) = seed_session(&st.db).await;
    // A rota é gated por permissão (R0.3): papel com `directory.manage` + binding.
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

    // Sem responsável → 400 missing_responsavel, nada criado.
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

    // Com responsável → 201 + vínculo admin no escopo do diretório.
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
    let yesterday = Utc::now() - Duration::hours(26);
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
