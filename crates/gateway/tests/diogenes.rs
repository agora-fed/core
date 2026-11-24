//! DIOGENES add-on through the real gateway: session cookie → `inject_identity` → the
//! `diogenes_addon::HostIdentity` adapter → module gate → role permission.
//!
//! Requires `DATABASE_URL` with the full migration chain applied (CI does both).
#![cfg(feature = "diogenes")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use tower::util::ServiceExt;
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;

async fn app() -> (Router, Db) {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a test database");
    let db = dsoc_db::connect(&url, 5).await.expect("connect");
    diogenes::migrate(&db).await.expect("diogenes schema");
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

/// A citizen with a public handle and a live session. Returns (citizen, cookie).
async fn citizen(db: &Db, org: Uuid, handle: &str) -> (Uuid, String) {
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
    .unwrap();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at, handle, display_name)
         VALUES ($1, $2, $3, 'email', $4, $5, $6)",
    )
    .bind(citizen)
    .bind(org)
    .bind(format!("sub-{}", citizen.simple()))
    .bind(now)
    .bind(handle)
    .bind(format!("Nome {handle}"))
    .execute(db)
    .await
    .unwrap();
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
    .bind(handle)
    .execute(db)
    .await
    .unwrap();
    (citizen, format!("dsoc_session={session}"))
}

async fn enable_module(db: &Db, org: Uuid) {
    sqlx::query(
        "INSERT INTO admin_feature_flag (id, org_id, key, enabled, created_at, updated_at)
         VALUES ($1, $2, 'module.diogenes', true, now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .execute(db)
    .await
    .unwrap();
}

/// Grant the `diogenes.rooms.create` permission through a real role.
async fn grant_create_rooms(db: &Db, org: Uuid, citizen: Uuid) {
    let role = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO user_role (id, org_id, name, position, permissions)
         VALUES ($1, $2, $3, 5, ARRAY['diogenes.rooms.create'])",
    )
    .bind(role)
    .bind(org)
    .bind(format!("Articulação {}", role.simple()))
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO citizen_role_binding (id, org_id, citizen_id, role_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(citizen)
    .bind(role)
    .execute(db)
    .await
    .unwrap();
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    extra: &[(&str, String)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    for (k, v) in extra {
        req = req.header(*k, v);
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn diogenes_through_the_real_identity_adapter() {
    let (app, db) = app().await;
    let org = Uuid::now_v7();
    let suffix = Uuid::now_v7().simple().to_string();
    let ana_handle = format!("ana{}", &suffix[20..]);
    let bia_handle = format!("bia{}", &suffix[20..]);
    let (ana, ana_cookie) = citizen(&db, org, &ana_handle).await;
    let (_bia, bia_cookie) = citizen(&db, org, &bia_handle).await;

    // A client cannot name itself: a spoofed caller header with no session is 401.
    let spoof = [
        ("x-dsoc-citizen-id", ana.to_string()),
        ("x-dsoc-org-id", org.to_string()),
    ];
    let (s, _) = call(&app, "GET", "/api/v1/diogenes/rooms", None, &spoof, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Module off (the default) in another org: 404 module_disabled. (The gate caches flags
    // for 30 s, so each org is checked in only one state.)
    let off_org = Uuid::now_v7();
    let (_cid, off_cookie) = citizen(&db, off_org, &format!("off{}", &suffix[20..])).await;
    let (s, body) = call(
        &app,
        "GET",
        "/api/v1/diogenes/rooms",
        Some(&off_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "module_disabled");

    enable_module(&db, org).await;
    // Module on, no permission: can list, cannot create.
    let (s, body) = call(
        &app,
        "GET",
        "/api/v1/diogenes/rooms",
        Some(&ana_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["can_create_rooms"], false);
    assert_eq!(body["data"]["display_name"], format!("Nome {ana_handle}"));
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/diogenes/rooms",
        Some(&ana_cookie),
        &[],
        Some(json!({"name": "Sala"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // With the role permission: create a room and add a member by handle.
    grant_create_rooms(&db, org, ana).await;
    let (s, room) = call(
        &app,
        "POST",
        "/api/v1/diogenes/rooms",
        Some(&ana_cookie),
        &[],
        Some(json!({"name": "Sala"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{room}");
    let room_id = room["data"]["id"].as_str().unwrap().to_owned();
    let (s, body) = call(
        &app,
        "POST",
        &format!("/api/v1/diogenes/rooms/{room_id}/members"),
        Some(&ana_cookie),
        &[],
        Some(json!({"handle": format!("@{}", bia_handle.to_uppercase()), "role": "viewer"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["display_name"], format!("Nome {bia_handle}"));
    let (s, body) = call(
        &app,
        "GET",
        &format!("/api/v1/diogenes/rooms/{room_id}"),
        Some(&bia_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"]["role"], "viewer");

    // The data sync is shared by the installation: with no DIOGENES_OPERATOR_ORG, no org
    // can trigger it by hand, whatever its roles.
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/diogenes/sync/groups",
        Some(&ana_cookie),
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
