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

    // Bia was told: the bell row arrives from the background task.
    let mut rings = 0;
    for _ in 0..50 {
        rings = bell(&db, _bia).await.len();
        if rings > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(rings, 1, "the new member gets one notification");
    // A member without a profile name shows as @handle, never as a bare id.
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

/// Boot wiring: `start` applies the schema and, with no `DIOGENES_SYNC_ENABLED`, starts no
/// scheduler; `start_with(.., true)` returns the scheduler task. The test aborts it before the
/// first poll (current-thread runtime, no await in between), so no request reaches the Câmara.
#[tokio::test]
async fn start_applies_the_schema_and_gates_the_scheduler() {
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
    assert!(std::env::var("DIOGENES_SYNC_ENABLED").is_err());
    dsoc_gateway::diogenes_addon::start(&state).await;
    let version: i32 = sqlx::query_scalar("SELECT max(version) FROM diogenes_migration")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(version >= 1);
    assert!(dsoc_gateway::diogenes_addon::start_with(&state, false)
        .await
        .is_none());
    let scheduler = dsoc_gateway::diogenes_addon::start_with(&state, true)
        .await
        .expect("scheduler task");
    scheduler.abort();
}

async fn credential(db: &Db, org: Uuid, citizen: Uuid, email: &str) {
    sqlx::query(
        "INSERT INTO auth_credential (id, citizen_id, org_id, email, password_hash, created_at)
         VALUES ($1, $2, $3, $4, 'x', now())",
    )
    .bind(Uuid::now_v7())
    .bind(citizen)
    .bind(org)
    .bind(email)
    .execute(db)
    .await
    .unwrap();
}

fn notice(org: Uuid, user: Uuid, by: Uuid) -> diogenes::MemberAdded {
    diogenes::MemberAdded {
        org_id: org,
        room_id: Uuid::now_v7(),
        room_name: "PL 4675/2025".into(),
        user_id: user,
        role: "editor".into(),
        added_by: by,
        added_by_name: "Ana".into(),
    }
}

async fn bell(db: &Db, citizen: Uuid) -> Vec<(String, Option<String>, Option<String>)> {
    sqlx::query_as(
        "SELECT kind, object_uri, object_preview FROM user_notification WHERE citizen_id = $1",
    )
    .bind(citizen)
    .fetch_all(db)
    .await
    .unwrap()
}

/// The notice writes the bell row and renders the e-mail from the editable template.
#[tokio::test]
async fn member_notice_rings_the_bell_and_renders_the_mail() {
    let (_app, db) = app().await;
    let org = Uuid::now_v7();
    let s = Uuid::now_v7().simple().to_string();
    let (ana, _) = citizen(&db, org, &format!("na{}", &s[20..])).await;
    let (bia, _) = citizen(&db, org, &format!("nb{}", &s[20..])).await;
    credential(&db, org, bia, &format!("bia{}@example.org", &s[20..])).await;

    let n = notice(org, bia, ana);
    let mail = dsoc_gateway::diogenes_notice::record(&db, "https://site.test/", &n)
        .await
        .unwrap()
        .expect("e-mail");
    assert_eq!(mail.to, format!("bia{}@example.org", &s[20..]));
    assert_eq!(mail.subject, "Ana adicionou você à sala \"PL 4675/2025\"");
    let url = format!("https://site.test/articulacao/#/sala/{}", n.room_id);
    assert!(mail.body.contains(&url), "{}", mail.body);
    assert!(mail.body.contains("como editor(a)"));
    assert!(mail
        .body
        .contains("https://site.test/configuracoes#preferencias"));

    let rows = bell(&db, bia).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "room_member");
    assert_eq!(rows[0].1.as_deref(), Some(url.as_str()));
    assert_eq!(
        rows[0].2.as_deref(),
        Some("Ana adicionou você à sala \"PL 4675/2025\" como editor(a)")
    );
}

/// Opt-out keeps the bell but sends no e-mail; no address sends no e-mail; a recipient of
/// another org gets nothing at all.
#[tokio::test]
async fn member_notice_respects_opt_out_address_and_org() {
    let (_app, db) = app().await;
    let org = Uuid::now_v7();
    let s = Uuid::now_v7().simple().to_string();
    let (ana, _) = citizen(&db, org, &format!("oa{}", &s[20..])).await;
    let (opted_out, _) = citizen(&db, org, &format!("ob{}", &s[20..])).await;
    credential(&db, org, opted_out, &format!("ob{}@example.org", &s[20..])).await;
    sqlx::query(
        "UPDATE citizen SET email_prefs = email_prefs || '{\"room_member\": false}' WHERE id = $1",
    )
    .bind(opted_out)
    .execute(&db)
    .await
    .unwrap();
    let got =
        dsoc_gateway::diogenes_notice::record(&db, "https://s.test", &notice(org, opted_out, ana))
            .await
            .unwrap();
    assert!(got.is_none());
    assert_eq!(bell(&db, opted_out).await.len(), 1);

    let (no_mail, _) = citizen(&db, org, &format!("oc{}", &s[20..])).await;
    let got =
        dsoc_gateway::diogenes_notice::record(&db, "https://s.test", &notice(org, no_mail, ana))
            .await
            .unwrap();
    assert!(got.is_none());
    assert_eq!(bell(&db, no_mail).await.len(), 1);

    let foreign_org = Uuid::now_v7();
    let (foreign, _) = citizen(&db, foreign_org, &format!("od{}", &s[20..])).await;
    credential(
        &db,
        foreign_org,
        foreign,
        &format!("od{}@example.org", &s[20..]),
    )
    .await;
    let got =
        dsoc_gateway::diogenes_notice::record(&db, "https://s.test", &notice(org, foreign, ana))
            .await
            .unwrap();
    assert!(got.is_none());
    assert!(bell(&db, foreign).await.is_empty());
}
