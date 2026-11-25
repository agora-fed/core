//! OAuth bearer tokens (Mastodon Client API) are limited to the scopes they were granted.
//!
//! `inject_identity` accepts a bearer as an alternate credential. Before this test the token's
//! `scopes` were never checked, so a `read` token could post, delete and vote on every route.
//!
//! Requires `DATABASE_URL` with the full migration chain applied (CI does both).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use tower::util::ServiceExt;
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;
use dsoc_gateway::mastodon_oauth;

async fn app() -> (Router, Db) {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a test database");
    let db = dsoc_db::connect(&url, 5).await.expect("connect");
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let bus = Arc::new(dsoc_events::PgEventBus::new(db.clone()));
    let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
    let state = AppState {
        db: db.clone(),
        bus,
        authz,
        clock,
        storage: None,
    };
    (dsoc_gateway::api_router(state), db)
}

/// A citizen in a fresh org plus a bearer token with `scopes`. Returns the raw token.
async fn token_with(db: &Db, scopes: &str) -> String {
    let org = Uuid::now_v7();
    let citizen = Uuid::now_v7();
    let now = Utc::now();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Test Org', $3)")
        .bind(org)
        .bind(format!("org-{}", org.simple()))
        .bind(now)
        .execute(db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at, handle)
         VALUES ($1, $2, $3, 'email', $4, $5)",
    )
    .bind(citizen)
    .bind(org)
    .bind(format!("sub-{}", citizen.simple()))
    .bind(now)
    .bind(format!("b{}", &citizen.simple().to_string()[20..]))
    .execute(db)
    .await
    .unwrap();
    let app = mastodon_oauth::register_application(
        db,
        "Scope test",
        vec!["urn:ietf:wg:oauth:2.0:oob".to_owned()],
        "read write follow push",
        None,
    )
    .await
    .unwrap();
    mastodon_oauth::issue_access_token(db, app.id, Some(citizen), scopes)
        .await
        .unwrap()
        .access_token
}

fn req(method: &str, uri: &str, token: &str, body: Option<&str>) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    match body {
        Some(json) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_owned())),
        None => b.body(Body::empty()),
    }
    .unwrap()
}

async fn status(app: &Router, r: Request<Body>) -> StatusCode {
    app.clone().oneshot(r).await.unwrap().status()
}

#[tokio::test]
async fn read_token_can_read_but_not_write() {
    let (app, db) = app().await;
    let token = token_with(&db, "read").await;

    let s = status(
        &app,
        req("GET", "/api/v1/accounts/verify_credentials", &token, None),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let s = status(
        &app,
        req(
            "POST",
            "/api/v1/statuses",
            &token,
            Some(r#"{"status":"oi"}"#),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let s = status(
        &app,
        req("POST", "/api/v1/lists", &token, Some(r#"{"title":"x"}"#)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn write_token_can_write() {
    let (app, db) = app().await;
    let token = token_with(&db, "read write").await;
    let s = status(
        &app,
        req("POST", "/api/v1/lists", &token, Some(r#"{"title":"x"}"#)),
    )
    .await;
    assert_ne!(s, StatusCode::FORBIDDEN);
    assert!(s.is_success(), "{s}");
}

#[tokio::test]
async fn push_only_token_cannot_read_the_account() {
    let (app, db) = app().await;
    let token = token_with(&db, "push").await;
    let s = status(
        &app,
        req("GET", "/api/v1/accounts/verify_credentials", &token, None),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn forbidden_body_is_mastodon_shaped() {
    let (app, db) = app().await;
    let token = token_with(&db, "read").await;
    let res = app
        .clone()
        .oneshot(req(
            "DELETE",
            "/api/v1/statuses/00000000-0000-0000-0000-000000000000",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 16)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"], "This action is outside the authorized scopes");
}
