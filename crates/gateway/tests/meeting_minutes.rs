//! SECURITY: a meeting's minutes can only be read or changed by an admin of the meeting's org.
//! Requires `DATABASE_URL`, like http_surface.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;
use tower::util::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "ata sigilosa";

async fn app() -> (Router, AppState) {
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
    (dsoc_gateway::api_router(st.clone()), st)
}

async fn new_org(db: &Db) -> Uuid {
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'Test', now())")
        .bind(org)
        .bind(format!("t-{}", org.simple()))
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// An e-mail verified citizen with a live session; returns the Cookie header.
async fn person(db: &Db, org: Uuid, admin: bool) -> String {
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at)
         VALUES ($1, $2, 'email', now())",
    )
    .bind(citizen)
    .bind(org)
    .execute(db)
    .await
    .expect("seed citizen");
    sqlx::query(
        "INSERT INTO auth_session (id, org_id, citizen_id, oidc_subject, issued_at, expires_at,
                                   public_handle, created_at)
         VALUES ($1, $2, $3, $4, now(), $5, $4, now())",
    )
    .bind(session)
    .bind(org)
    .bind(citizen)
    .bind(format!("u-{}", citizen.simple()))
    .bind(Utc::now() + Duration::hours(1))
    .execute(db)
    .await
    .expect("seed session");
    if admin {
        sqlx::query(
            "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
             VALUES ($1, $2, $3, 'admin', now())",
        )
        .bind(Uuid::now_v7())
        .bind(org)
        .bind(citizen)
        .execute(db)
        .await
        .expect("seed admin");
    }
    format!("dsoc_session={session}")
}

async fn meeting(db: &Db, org: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO meeting (id, org_id, title, location, starts_at, minutes, created_at)
         VALUES ($1, $2, 'Assembleia', 'Sede', now(), $3, now())",
    )
    .bind(id)
    .bind(org)
    .bind(SECRET)
    .execute(db)
    .await
    .expect("seed meeting");
    id
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, cookie);
    }
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let req = req
        .body(Body::from(body.unwrap_or_default().to_owned()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

const NEW_MINUTES: &str = r#"{"minutes":"texto trocado"}"#;

async fn stored_minutes(db: &Db, id: Uuid) -> String {
    sqlx::query_scalar("SELECT minutes FROM meeting WHERE id = $1")
        .bind(id)
        .fetch_one(db)
        .await
        .expect("read minutes")
}

#[tokio::test]
async fn public_meeting_reads_never_carry_the_minutes() {
    let (app, st) = app().await;
    let org = new_org(&st.db).await;
    let id = meeting(&st.db, org).await;

    for uri in [
        format!("/api/v1/meetings/{id}"),
        format!("/api/v1/meetings?org_id={org}"),
    ] {
        let (status, body) = call(&app, "GET", &uri, None, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(body.contains("Assembleia"), "{uri}: {body}");
        assert!(!body.contains(SECRET), "{uri}: {body}");
    }
}

#[tokio::test]
async fn only_an_admin_of_the_org_reaches_the_minutes() {
    let (app, st) = app().await;
    let org = new_org(&st.db).await;
    let other_org = new_org(&st.db).await;
    let id = meeting(&st.db, org).await;
    let uri = format!("/api/v1/meetings/{id}/minutes");

    let member = person(&st.db, org, false).await;
    let foreign_admin = person(&st.db, other_org, true).await;
    let cases = [
        (None, StatusCode::UNAUTHORIZED),
        (Some(member.as_str()), StatusCode::FORBIDDEN),
        (Some(foreign_admin.as_str()), StatusCode::NOT_FOUND),
    ];
    for (cookie, expected) in cases {
        let (status, body) = call(&app, "GET", &uri, cookie, None).await;
        assert_eq!(status, expected, "read: {body}");
        assert!(!body.contains(SECRET), "read: {body}");
        let (status, body) = call(&app, "PUT", &uri, cookie, Some(NEW_MINUTES)).await;
        assert_eq!(status, expected, "write: {body}");
        assert_eq!(stored_minutes(&st.db, id).await, SECRET);
    }

    let admin = person(&st.db, org, true).await;
    let (status, body) = call(&app, "GET", &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(SECRET), "{body}");
    let (status, body) = call(&app, "PUT", &uri, Some(&admin), Some(NEW_MINUTES)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(stored_minutes(&st.db, id).await, "texto trocado");
}
