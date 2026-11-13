//! Public petitions: members create and sign; visitors sign with name, e-mail and a
//! proof-of-work, and the signature counts only after the e-mail link. Requires
//! `DATABASE_URL`. Runs in `local` verification mode with no SMTP and no provider.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use base64::Engine as _;
use chrono::{Duration, Utc};
use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::util::ServiceExt;
use uuid::Uuid;

/// The instance org that anonymous requests fall back to.
const DEFAULT_ORG: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

async fn app() -> (Router, AppState) {
    // Every test sets the same values before the first request, so the configuration
    // the module reads once is the same whichever test runs first.
    std::env::set_var("ALTCHA_HMAC_KEY", "test-altcha-key");
    std::env::remove_var("SMTP_HOST");
    std::env::remove_var("PETITIONS_EMAIL_VERIFICATION");
    std::env::remove_var("PETITIONS_SSO_PROVISIONING");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a test database");
    let db = dsoc_db::connect(&url, 5).await.expect("connect");
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let bus = Arc::new(dsoc_events::PgEventBus::new(db.clone()));
    let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
    let st = AppState {
        db,
        bus,
        authz,
        clock,
        storage: None,
    };
    module_on(&st.db, DEFAULT_ORG).await;
    (dsoc_gateway::api_router(st.clone()), st)
}

async fn module_on(db: &Db, org: Uuid) {
    sqlx::query(
        "INSERT INTO admin_org (org_id, is_active, created_at) VALUES ($1, true, now())
         ON CONFLICT DO NOTHING",
    )
    .bind(org)
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO admin_feature_flag (id, org_id, key, enabled, created_at, updated_at)
         VALUES ($1, $2, 'module.initiatives', true, now(), now())
         ON CONFLICT (org_id, key) DO UPDATE SET enabled = true",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .execute(db)
    .await
    .unwrap();
}

async fn person(db: &Db, admin: bool) -> (Uuid, String) {
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at, handle, display_name)
         VALUES ($1, $2, 'email', now(), $3, 'Pessoa Teste')",
    )
    .bind(citizen)
    .bind(DEFAULT_ORG)
    .bind(format!("p{}", &citizen.simple().to_string()[20..]))
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_session (id, org_id, citizen_id, oidc_subject, issued_at, expires_at,
                                   public_handle, created_at)
         VALUES ($1, $2, $3, $4, now(), $5, $4, now())",
    )
    .bind(session)
    .bind(DEFAULT_ORG)
    .bind(citizen)
    .bind(format!("u-{}", citizen.simple()))
    .bind(Utc::now() + Duration::hours(1))
    .execute(db)
    .await
    .unwrap();
    if admin {
        sqlx::query(
            "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
             VALUES ($1, $2, $3, 'admin', now())",
        )
        .bind(Uuid::now_v7())
        .bind(DEFAULT_ORG)
        .bind(citizen)
        .execute(db)
        .await
        .unwrap();
    }
    (citizen, format!("dsoc_session={session}"))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, cookie);
    }
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn new_petition(app: &Router, cookie: &str, threshold: i32) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/api/v1/petitions",
        Some(cookie),
        Some(json!({
            "title": "Mais ciclovias no centro",
            "body": "Pedimos à prefeitura que construa ciclovias seguras no centro.",
            "threshold": threshold
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"]["id"].as_str().unwrap().to_owned()
}

/// Ask for a challenge and solve it like the browser does.
async fn solved_altcha(app: &Router) -> String {
    let (status, c) = call(app, "GET", "/api/v1/petitions/altcha", None, None).await;
    assert_eq!(status, StatusCode::OK, "{c}");
    let salt = c["salt"].as_str().unwrap();
    let target = c["challenge"].as_str().unwrap();
    let max = c["maxnumber"].as_u64().unwrap();
    let number = (0..=max)
        .find(|n| {
            let digest = Sha256::digest(format!("{salt}{n}").as_bytes());
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                == target
        })
        .unwrap();
    let solution = json!({
        "algorithm": c["algorithm"], "challenge": c["challenge"], "number": number,
        "salt": c["salt"], "signature": c["signature"]
    });
    base64::engine::general_purpose::STANDARD.encode(solution.to_string())
}

#[tokio::test]
async fn members_create_list_and_sign_once() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let (_, other) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 2).await;

    let (status, list) = call(&app, "GET", "/api/v1/petitions", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == id));

    let uri = format!("/api/v1/petitions/{id}/sign");
    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(&other),
        Some(json!({"show_name": true, "city": "Recife"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["signature_count"], 1);
    let (status, body) = call(&app, "POST", &uri, Some(&other), None).await;
    assert_eq!(status, StatusCode::OK, "a repeat sign is a no-op: {body}");
    assert_eq!(body["data"]["signature_count"], 1);
    let (_, body) = call(&app, "POST", &uri, Some(&author), None).await;
    assert_eq!(body["data"]["reached"], true);

    let (_, detail) = call(
        &app,
        "GET",
        &format!("/api/v1/petitions/{id}"),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(detail["data"]["signed_by_me"], true);
    assert_eq!(detail["data"]["can_close"], false);
    let signers = detail["data"]["signers"].as_array().unwrap();
    assert_eq!(signers.len(), 1, "only the signer who asked is shown");
    assert_eq!(signers[0]["city"], "Recife");
}

#[tokio::test]
async fn visitors_cannot_create_and_drafts_are_checked() {
    let (app, st) = app().await;
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/petitions",
        None,
        Some(json!({"title": "Título bom", "body": "x".repeat(30), "threshold": 10})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, member) = person(&st.db, false).await;
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/petitions",
        Some(&member),
        Some(json!({"title": "x", "body": "curto", "threshold": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn only_the_author_or_an_admin_closes_and_only_an_admin_hides() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let (_, other) = person(&st.db, false).await;
    let (_, admin) = person(&st.db, true).await;
    let id = new_petition(&app, &author, 10).await;
    let uri = format!("/api/v1/petitions/{id}");

    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&other),
        Some(json!({"status": "closed"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&author),
        Some(json!({"status": "hidden"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&author),
        Some(json!({"status": "closed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&app, "POST", &format!("{uri}/sign"), Some(&other), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({"status": "hidden"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", &uri, None, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "hidden is gone for the public"
    );
    let (status, _) = call(&app, "GET", &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "admins still see it");
    let (status, _) = call(&app, "GET", "/api/v1/petitions?status=hidden", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_public_signature_needs_proof_of_work_consent_and_mail() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let uri = format!("/api/v1/petitions/{id}/public-signatures");
    let email = format!("ana-{}@example.org", Uuid::now_v7().simple());

    let (_, settings) = call(&app, "GET", "/api/v1/petitions/settings", None, None).await;
    assert_eq!(settings["data"]["anonymous_signing"], true);
    assert_eq!(settings["data"]["verification"], "local");

    let form = |altcha: &str, consent: bool| json!({"name": "Ana Souza", "email": email, "consent": consent, "altcha": altcha});
    let (status, _) = call(&app, "POST", &uri, None, Some(form("garbage", true))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no proof of work");
    let altcha = solved_altcha(&app).await;
    let (status, _) = call(&app, "POST", &uri, None, Some(form(&altcha, false))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no consent");

    // Valid request, but this test server has no SMTP: the visitor is told, nothing counts.
    let (status, body) = call(&app, "POST", &uri, None, Some(form(&altcha, true))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "mail_unavailable");
    let mut replay = form(&altcha, true);
    replay["email"] = json!(format!("outra-{}@example.org", Uuid::now_v7().simple()));
    let (status, _) = call(&app, "POST", &uri, None, Some(replay)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a solution is single-use"
    );

    let (_, detail) = call(&app, "GET", &format!("/api/v1/petitions/{id}"), None, None).await;
    assert_eq!(detail["data"]["signature_count"], 0);
}

async fn pending_with_token(db: &Db, petition: &str, email: &str, token: &str) {
    sqlx::query(
        "INSERT INTO initiative_pending_signature
             (id, initiative_id, email, name, city, show_name, token_hash, created_at, last_sent_at,
              expires_at)
         VALUES ($1, $2, $3, 'Bia Lima', 'Olinda', true, $4, now(), now(), now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::parse_str(petition).unwrap())
    .bind(email)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_link_counts_the_signature_once_and_creates_the_account() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let email = format!("bia-{}@example.org", Uuid::now_v7().simple());
    let token = format!("tok-{}", Uuid::now_v7().simple());
    pending_with_token(&st.db, &id, &email, &token).await;

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["signature_count"], 1);
    assert_eq!(
        body["data"]["account_created"], false,
        "no provider configured"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "single use: {body}");

    let citizen: (String, Option<String>) = sqlx::query_as(
        "SELECT c.verification_level, c.oidc_subject FROM citizen c
           JOIN auth_credential a ON a.citizen_id = c.id WHERE a.email = $1",
    )
    .bind(&email)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(citizen, ("email".to_owned(), None));
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM initiative_pending_signature WHERE email = $1")
            .bind(&email)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(left, 0, "the transient e-mail copy is gone");

    // The same address signing a second petition reuses the account.
    let second = new_petition(&app, &author, 10).await;
    let token2 = format!("tok-{}", Uuid::now_v7().simple());
    pending_with_token(&st.db, &second, &email, &token2).await;
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"token": token2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_credential WHERE email = $1")
        .bind(&email)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[tokio::test]
async fn an_expired_link_does_nothing() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let token = format!("tok-{}", Uuid::now_v7().simple());
    pending_with_token(&st.db, &id, "old@example.org", &token).await;
    sqlx::query("UPDATE initiative_pending_signature SET expires_at = now() - interval '1 minute' WHERE email = 'old@example.org'")
        .execute(&st.db)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"pending": Uuid::now_v7(), "user": "1", "code": "X"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "sso proof in local mode is refused"
    );
}

#[tokio::test]
async fn the_same_link_opened_twice_at_once_counts_once() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let email = format!("par-{}@example.org", Uuid::now_v7().simple());
    let token = format!("tok-{}", Uuid::now_v7().simple());
    pending_with_token(&st.db, &id, &email, &token).await;
    let body = json!({ "token": token });
    let (a, b) = tokio::join!(
        call(
            &app,
            "POST",
            "/api/v1/petitions/confirm",
            None,
            Some(body.clone())
        ),
        call(&app, "POST", "/api/v1/petitions/confirm", None, Some(body)),
    );
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::GONE], "{a:?} {b:?}");
    let count: i64 = sqlx::query_scalar("SELECT signature_count FROM initiative WHERE id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_credential WHERE email = $1")
        .bind(&email)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[tokio::test]
async fn an_address_waits_before_another_email() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let email = format!("wait-{}@example.org", Uuid::now_v7().simple());
    pending_with_token(
        &st.db,
        &id,
        &email,
        &format!("tok-{}", Uuid::now_v7().simple()),
    )
    .await;
    let altcha = solved_altcha(&app).await;
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v1/petitions/{id}/public-signatures"),
        None,
        Some(json!({"name": "Ana Souza", "email": email, "consent": true, "altcha": altcha})),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "cooldown");
}

#[tokio::test]
async fn the_raw_initiative_routes_are_not_mounted() {
    let (app, _st) = app().await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/initiatives?org_id={DEFAULT_ORG}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unverified_member_cannot_sign() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let (citizen, cookie) = person(&st.db, false).await;
    sqlx::query("UPDATE citizen SET verification_level = 'anonymous' WHERE id = $1")
        .bind(citizen)
        .execute(&st.db)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/petitions/{id}/sign"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn erasure_removes_the_public_name_but_keeps_the_count() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let (citizen, cookie) = person(&st.db, false).await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/petitions/{id}/sign"),
        Some(&cookie),
        Some(json!({"show_name": true, "city": "Recife"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/me/lgpd/delete-account",
        Some(&cookie),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT public_name, city FROM initiative_signature WHERE citizen_id = $1")
            .bind(citizen)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(row, (None, None));
    let (_, detail) = call(&app, "GET", &format!("/api/v1/petitions/{id}"), None, None).await;
    assert_eq!(detail["data"]["signature_count"], 1);
}

#[tokio::test]
async fn an_unconfirmed_account_with_the_address_is_never_reused() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let id = new_petition(&app, &author, 10).await;
    let email = format!("squat-{}@example.org", Uuid::now_v7().simple());
    // Someone typed the address into a password sign-up and never confirmed it.
    let (squatter, _) = person(&st.db, false).await;
    sqlx::query("UPDATE citizen SET verification_level = 'anonymous' WHERE id = $1")
        .bind(squatter)
        .execute(&st.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth_credential (id, citizen_id, org_id, email, password_hash, cpf_status, created_at)
         VALUES ($1, $2, $3, $4, 'x', 'unverified', now())",
    )
    .bind(Uuid::now_v7())
    .bind(squatter)
    .bind(DEFAULT_ORG)
    .bind(&email)
    .execute(&st.db)
    .await
    .unwrap();
    let token = format!("tok-{}", Uuid::now_v7().simple());
    pending_with_token(&st.db, &id, &email, &token).await;
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/petitions/confirm",
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let signer: Uuid =
        sqlx::query_scalar("SELECT citizen_id FROM initiative_signature WHERE initiative_id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_ne!(
        signer, squatter,
        "the signature must not land in the squatter's account"
    );
    let level: String = sqlx::query_scalar("SELECT verification_level FROM citizen WHERE id = $1")
        .bind(squatter)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(level, "anonymous", "the squatter is not promoted");
}

#[tokio::test]
async fn the_author_cannot_undo_what_an_admin_closed() {
    let (app, st) = app().await;
    let (_, author) = person(&st.db, false).await;
    let (_, admin) = person(&st.db, true).await;
    let id = new_petition(&app, &author, 10).await;
    let uri = format!("/api/v1/petitions/{id}");
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({"status": "closed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&author),
        Some(json!({"status": "open"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({"status": "open"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&author),
        Some(json!({"status": "closed"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "after an admin reopens, the author decides again"
    );
}
