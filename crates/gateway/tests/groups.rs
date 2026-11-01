//! SECURITY and behaviour of the optional groups module. Requires `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;
use serde_json::Value;
use tower::util::ServiceExt;
use uuid::Uuid;

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

async fn org(db: &Db, on: bool) -> Uuid {
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, 'T', now())")
        .bind(org)
        .bind(format!("g-{}", org.simple()))
        .execute(db)
        .await
        .unwrap();
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
         VALUES ($1, $2, 'module.groups', $3, now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(on)
    .execute(db)
    .await
    .unwrap();
    org
}

async fn person(db: &Db, org: Uuid, admin: bool) -> (Uuid, String) {
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at, handle, display_name)
         VALUES ($1, $2, 'email', now(), $3, 'Pessoa Teste')",
    )
    .bind(citizen)
    .bind(org)
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
    .bind(org)
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
        .bind(org)
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

async fn new_group(app: &Router, admin: &str, entry: &str) -> (String, String) {
    let slug = format!("grupo{}", &Uuid::now_v7().simple().to_string()[20..]);
    let (status, body) = call(
        app,
        "POST",
        "/api/v1/groups",
        Some(admin),
        Some(serde_json::json!({"slug": slug, "name": "Moderação", "entry": entry})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (body["data"]["id"].as_str().unwrap().to_owned(), slug)
}

#[tokio::test]
async fn module_off_means_the_routes_do_not_exist() {
    let (app, st) = app().await;
    let org = org(&st.db, false).await;
    let (_, cookie) = person(&st.db, org, true).await;
    let (status, body) = call(&app, "GET", "/api/v1/groups", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "module_disabled");
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&cookie),
        Some(serde_json::json!({"slug": "abc", "name": "X"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn module_is_off_by_default_even_without_signing_in() {
    let (app, _st) = app().await;
    let (status, _) = call(&app, "GET", "/api/v1/groups", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_admins_create_groups() {
    let (app, st) = app().await;
    let org = org(&st.db, true).await;
    let (_, cookie) = person(&st.db, org, false).await;
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&cookie),
        Some(serde_json::json!({"slug": "abc", "name": "X"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn approval_flow_and_member_privacy() {
    let (app, st) = app().await;
    let org = org(&st.db, true).await;
    let (_, admin) = person(&st.db, org, true).await;
    let (candidate, candidate_cookie) = person(&st.db, org, false).await;
    let (_, outsider) = person(&st.db, org, false).await;
    let (id, slug) = new_group(&app, &admin, "approval").await;
    let members = format!("/api/v1/groups/{id}/members");

    let (status, _) = call(&app, "GET", &members, Some(&outsider), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "outsiders don't see who belongs"
    );

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{id}/join"),
        Some(&candidate_cookie),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "pending");

    let (status, _) = call(&app, "GET", &members, Some(&candidate_cookie), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a pending request isn't membership"
    );

    // Only coordination approves.
    let approve = format!("/api/v1/groups/{id}/members/{candidate}");
    let (status, _) = call(
        &app,
        "PATCH",
        &approve,
        Some(&candidate_cookie),
        Some(serde_json::json!({"status": "active"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        "PATCH",
        &approve,
        Some(&admin),
        Some(serde_json::json!({"status": "active"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(&app, "GET", &members, Some(&candidate_cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert!(!body.to_string().contains('@'), "no e-mail in the list");

    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/groups/by-slug/{slug}"),
        Some(&candidate_cookie),
        None,
    )
    .await;
    assert_eq!(body["data"]["my_status"], "active");
}

#[tokio::test]
async fn the_group_forum_follows_active_membership() {
    let (app, st) = app().await;
    let org = org(&st.db, true).await;
    let (_, admin) = person(&st.db, org, true).await;
    let (member, member_cookie) = person(&st.db, org, false).await;
    let (id, _) = new_group(&app, &admin, "open").await;

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{id}/forum"),
        Some(&admin),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let path = body["data"]["forum_path"].as_str().unwrap().to_owned();
    let forum: Uuid =
        sqlx::query_scalar("SELECT forum_id FROM community_group WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    let readable = |citizen: Uuid| {
        let db = st.db.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT forum_readable($1, $2)")
                .bind(forum)
                .bind(citizen)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    let visibility: (String, bool) =
        sqlx::query_as("SELECT visibility, federated FROM forum WHERE full_path = $1")
            .bind(&path)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(visibility, ("list".to_owned(), false));
    assert!(!readable(member).await);

    call(
        &app,
        "POST",
        &format!("/api/v1/groups/{id}/join"),
        Some(&member_cookie),
        Some(serde_json::json!({})),
    )
    .await;
    assert!(readable(member).await, "joining opens the forum");

    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/groups/{id}/join"),
        Some(&member_cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!readable(member).await, "leaving closes the forum");
}

#[tokio::test]
async fn a_group_always_keeps_a_coordinator() {
    let (app, st) = app().await;
    let org = org(&st.db, true).await;
    let (admin_id, admin) = person(&st.db, org, true).await;
    let (id, _) = new_group(&app, &admin, "open").await;
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/groups/{id}/join"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/groups/{id}/members/{admin_id}"),
        Some(&admin),
        Some(serde_json::json!({"role": "member"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn person_search_needs_coordination_and_stays_narrow() {
    let (app, st) = app().await;
    let org = org(&st.db, true).await;
    let (_, admin) = person(&st.db, org, true).await;
    let (_, plain) = person(&st.db, org, false).await;
    let (id, _) = new_group(&app, &admin, "open").await;
    let uri = |q: &str| format!("/api/v1/groups/{id}/people?q={q}");

    let (status, _) = call(&app, "GET", &uri("pessoa"), Some(&plain), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, body) = call(&app, "GET", &uri("pe"), Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0, "too short");
    let (_, body) = call(&app, "GET", &uri("%25%25%25"), Some(&admin), None).await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "wildcards are literal"
    );
    let (_, body) = call(&app, "GET", &uri("pessoa"), Some(&admin), None).await;
    let found = body["data"].as_array().unwrap();
    assert!(!found.is_empty());
    assert!(found[0].get("email").is_none());
}

#[tokio::test]
async fn groups_of_another_org_are_not_found() {
    let (app, st) = app().await;
    let a = org(&st.db, true).await;
    let b = org(&st.db, true).await;
    let (_, admin_a) = person(&st.db, a, true).await;
    let (_, admin_b) = person(&st.db, b, true).await;
    let (id, _) = new_group(&app, &admin_a, "open").await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/groups/{id}/join"),
        Some(&admin_b),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
