//! SECURITY: a restricted forum must look like it does not exist to anyone without access,
//! on every path that shows forum content. Requires `DATABASE_URL`, like http_surface.

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

const ORG: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

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
    sqlx::query(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, 'default-test', 'Test', now())
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(ORG)
    .execute(&st.db)
    .await
    .expect("seed org");
    (dsoc_gateway::api_router(st.clone()), st)
}

/// A citizen with a live session; returns (id, Cookie header).
async fn person(db: &Db) -> (Uuid, String) {
    let citizen = Uuid::now_v7();
    let session = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, verification_level, created_at)
         VALUES ($1, $2, 'email', now())",
    )
    .bind(citizen)
    .bind(ORG)
    .execute(db)
    .await
    .expect("seed citizen");
    sqlx::query(
        "INSERT INTO auth_session (id, org_id, citizen_id, oidc_subject, issued_at, expires_at,
                                   public_handle, created_at)
         VALUES ($1, $2, $3, $4, now(), $5, $4, now())",
    )
    .bind(session)
    .bind(ORG)
    .bind(citizen)
    .bind(format!("u-{}", citizen.simple()))
    .bind(Utc::now() + Duration::hours(1))
    .execute(db)
    .await
    .expect("seed session");
    (citizen, format!("dsoc_session={session}"))
}

struct Fixture {
    forum: Uuid,
    path: String,
    topic: Uuid,
    comment: Uuid,
    title: String,
}

async fn forum(db: &Db, author: Uuid, visibility: &str) -> Fixture {
    let forum = Uuid::now_v7();
    let topic = Uuid::now_v7();
    let comment = Uuid::now_v7();
    let path = format!("acesso-{}", forum.simple());
    let title = format!("Assunto sigiloso {}", topic.simple());
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, federated, visibility)
         VALUES ($1, $2, $3, $3, $3, 'comunitario', false, $4)",
    )
    .bind(forum)
    .bind(ORG)
    .bind(&path)
    .bind(visibility)
    .execute(db)
    .await
    .expect("seed forum");
    sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
         VALUES ($1, $2, $3, $4, 'corpo sigiloso', now())",
    )
    .bind(topic)
    .bind(forum)
    .bind(author)
    .bind(&title)
    .execute(db)
    .await
    .expect("seed topic");
    sqlx::query(
        "INSERT INTO forum_topic_comment (id, topic_id, author_id, body, created_at)
         VALUES ($1, $2, $3, 'argumento sigiloso', now())",
    )
    .bind(comment)
    .bind(topic)
    .bind(author)
    .execute(db)
    .await
    .expect("seed comment");
    Fixture {
        forum,
        path,
        topic,
        comment,
        title,
    }
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

/// Every read of a forum's content, for someone who can't see it.
async fn assert_hidden(app: &Router, f: &Fixture, cookie: Option<&str>) {
    let reads = [
        format!("/api/v1/f/tree?path={}", f.path),
        format!("/api/v1/f/topics?path={}", f.path),
        format!("/api/v1/f/topics/{}", f.topic),
        format!("/api/v1/f/topics/{}/consensus", f.topic),
    ];
    for uri in &reads {
        let (status, body) = call(app, "GET", uri, cookie, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(!body.contains("sigiloso"), "{uri}: {body}");
    }
    let (_, recent) = call(app, "GET", "/api/v1/f/recent?limit=50", cookie, None).await;
    assert!(!recent.contains(&f.title), "recent feed leaked");
    let (_, roots) = call(app, "GET", "/api/v1/f/tree", cookie, None).await;
    assert!(!roots.contains(&f.path), "forum list leaked");
}

async fn assert_visible(app: &Router, f: &Fixture, cookie: &str) {
    for uri in [
        format!("/api/v1/f/topics?path={}", f.path),
        format!("/api/v1/f/topics/{}", f.topic),
        format!("/api/v1/f/topics/{}/consensus", f.topic),
    ] {
        let (status, _) = call(app, "GET", &uri, Some(cookie), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let (_, recent) = call(app, "GET", "/api/v1/f/recent?limit=50", Some(cookie), None).await;
    assert!(
        recent.contains(&f.title),
        "reader should see it in the feed"
    );
    let (_, roots) = call(app, "GET", "/api/v1/f/tree", Some(cookie), None).await;
    assert!(roots.contains(&f.path), "reader should see it in the list");
}

async fn assert_writes_hidden(app: &Router, f: &Fixture, cookie: &str) {
    let (status, _) = call(
        app,
        "POST",
        "/api/v1/f/topics",
        Some(cookie),
        Some(&format!(
            r#"{{"path":"{}","title":"Um titulo valido","body":"Um corpo valido para o teste"}}"#,
            f.path
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "create topic");
    for (uri, body) in [
        (
            format!("/api/v1/f/topics/{}/vote", f.topic),
            r#"{"stance":"favor"}"#,
        ),
        (
            format!("/api/v1/f/topics/{}/comments", f.topic),
            r#"{"body":"Um argumento valido para o teste"}"#,
        ),
        (
            format!("/api/v1/f/comments/{}/vote", f.comment),
            r#"{"stance":"favor"}"#,
        ),
    ] {
        let (status, _) = call(app, "POST", &uri, Some(cookie), Some(body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn public_forum_stays_readable_by_everyone() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let f = forum(&st.db, author, "public").await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{}", f.topic),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, recent) = call(&app, "GET", "/api/v1/f/recent?limit=50", None, None).await;
    assert!(recent.contains(&f.title));
}

#[tokio::test]
async fn members_forum_is_for_signed_in_accounts() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let (_, reader) = person(&st.db).await;
    let f = forum(&st.db, author, "members").await;
    assert_hidden(&app, &f, None).await;
    assert_visible(&app, &f, &reader).await;
}

#[tokio::test]
async fn list_forum_is_only_for_its_members() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let (member, member_cookie) = person(&st.db).await;
    let (moderator, moderator_cookie) = person(&st.db).await;
    let (_, outsider) = person(&st.db).await;
    let f = forum(&st.db, author, "list").await;
    sqlx::query("INSERT INTO forum_member (forum_id, citizen_id) VALUES ($1, $2)")
        .bind(f.forum)
        .bind(member)
        .execute(&st.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO forum_moderator (forum_id, citizen_id) VALUES ($1, $2)")
        .bind(f.forum)
        .bind(moderator)
        .execute(&st.db)
        .await
        .unwrap();

    assert_hidden(&app, &f, None).await;
    assert_hidden(&app, &f, Some(&outsider)).await;
    assert_writes_hidden(&app, &f, &outsider).await;
    assert_visible(&app, &f, &member_cookie).await;
    assert_visible(&app, &f, &moderator_cookie).await;

    // Losing membership takes effect immediately.
    sqlx::query("DELETE FROM forum_member WHERE forum_id = $1 AND citizen_id = $2")
        .bind(f.forum)
        .bind(member)
        .execute(&st.db)
        .await
        .unwrap();
    assert_hidden(&app, &f, Some(&member_cookie)).await;
}

#[tokio::test]
async fn children_inherit_the_restriction_of_their_parent() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let parent = forum(&st.db, author, "list").await;
    let child = forum(&st.db, author, "public").await;
    sqlx::query("UPDATE forum SET parent_id = $1 WHERE id = $2")
        .bind(parent.forum)
        .bind(child.forum)
        .execute(&st.db)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{}", child.topic),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_suspended_member_loses_access() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let (member, cookie) = person(&st.db).await;
    let f = forum(&st.db, author, "members").await;
    sqlx::query("UPDATE citizen SET suspended_at = now() WHERE id = $1")
        .bind(member)
        .execute(&st.db)
        .await
        .unwrap();
    assert_hidden(&app, &f, Some(&cookie)).await;
}

#[tokio::test]
async fn database_refuses_a_federated_restricted_forum() {
    let (_, st) = app().await;
    let (author, _) = person(&st.db).await;
    let f = forum(&st.db, author, "list").await;
    let res = sqlx::query("UPDATE forum SET federated = true WHERE id = $1")
        .bind(f.forum)
        .execute(&st.db)
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn federation_never_serves_a_restricted_forum() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let f = forum(&st.db, author, "list").await;
    for uri in [
        format!(
            "/.well-known/webfinger?resource=acct:{}@test.example",
            f.path
        ),
        format!("/actors/{}", f.path),
        format!("/actors/{}/objects/{}", f.path, f.topic),
    ] {
        let req = Request::get(&uri)
            .header(header::ACCEPT, "application/activity+json")
            .header(header::HOST, "test.example")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            !String::from_utf8_lossy(&body).contains("sigiloso"),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_forum_made_private_stops_announcing() {
    let (_, st) = app().await;
    let (author, _) = person(&st.db).await;
    let f = forum(&st.db, author, "public").await;
    sqlx::query("UPDATE forum SET federated = true WHERE id = $1")
        .bind(f.forum)
        .execute(&st.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forum_follower (forum_id, remote_actor_url, remote_inbox_url, accepted_at, created_at)
         VALUES ($1, 'https://r.invalid/u', 'https://r.invalid/inbox', now(), now())",
    )
    .bind(f.forum)
    .execute(&st.db)
    .await
    .unwrap();
    let queued = |topic: Uuid| {
        let db = st.db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM forum_announce_delivery WHERE topic_id = $1 AND sent_at IS NULL",
            )
            .bind(topic)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    // Control: while public it does queue, so the assertion below is meaningful.
    dsoc_gateway::forum_federation::announce_sweep(&st.db, "https://test.invalid").await;
    assert_eq!(queued(f.topic).await, 1);

    sqlx::query("UPDATE forum SET federated = false, visibility = 'list' WHERE id = $1")
        .bind(f.forum)
        .execute(&st.db)
        .await
        .unwrap();
    let topic = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
         VALUES ($1, $2, $3, 'novo', 'novo', now())",
    )
    .bind(topic)
    .bind(f.forum)
    .bind(author)
    .execute(&st.db)
    .await
    .unwrap();
    dsoc_gateway::forum_federation::announce_sweep(&st.db, "https://test.invalid").await;
    assert_eq!(queued(topic).await, 0);
    let attempts: i32 =
        sqlx::query_scalar("SELECT attempts FROM forum_announce_delivery WHERE topic_id = $1")
            .bind(f.topic)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(attempts <= 1, "a queued announce must not keep going out");
}

#[tokio::test]
async fn representatives_of_a_hidden_topic_are_not_found() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let f = forum(&st.db, author, "list").await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/topics/{}/representatives", f.topic),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn moderation_routes_do_not_reveal_hidden_content() {
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let (_, outsider) = person(&st.db).await;
    let f = forum(&st.db, author, "list").await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/f/topics/{}/remove", f.topic),
        Some(&outsider),
        Some("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_manages_forums_and_members() {
    let (app, st) = app().await;
    let (admin, cookie) = person(&st.db).await;
    let (member, member_cookie) = person(&st.db).await;
    sqlx::query(
        "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
         VALUES ($1, $2, $3, 'admin', now())",
    )
    .bind(Uuid::now_v7())
    .bind(ORG)
    .bind(admin)
    .execute(&st.db)
    .await
    .unwrap();
    let slug = format!("novo-{}", &Uuid::now_v7().simple().to_string()[20..]);

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/admin/forums",
        Some(&member_cookie),
        Some(&format!(r#"{{"slug":"{slug}","name":"X"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/admin/forums",
        Some(&cookie),
        Some(&format!(
            r#"{{"slug":"{slug}","name":"Novo","visibility":"list","federated":true}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/admin/forums",
        Some(&cookie),
        Some(&format!(
            r#"{{"slug":"{slug}","name":"Novo","visibility":"list"}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id: Uuid = serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let topics = format!("/api/v1/f/topics?path={slug}");
    let (status, _) = call(&app, "GET", &topics, Some(&member_cookie), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/admin/forums/{id}/members"),
        Some(&cookie),
        Some(&format!(r#"{{"citizen_id":"{member}"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(&app, "GET", &topics, Some(&member_cookie), None).await;
    assert_eq!(status, StatusCode::OK);

    // A public forum can federate; restricting it stops that in the same statement.
    let patch = format!("/api/v1/admin/forums/{id}");
    let (status, body) = call(
        &app,
        "PATCH",
        &patch,
        Some(&cookie),
        Some(r#"{"federated":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, _) = call(
        &app,
        "PATCH",
        &patch,
        Some(&cookie),
        Some(r#"{"visibility":"public","federated":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PATCH",
        &patch,
        Some(&cookie),
        Some(r#"{"visibility":"members"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let federated: bool = sqlx::query_scalar("SELECT federated FROM forum WHERE id = $1")
        .bind(id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert!(!federated);

    let (status, _) = call(&app, "DELETE", &patch, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", &topics, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn share_previews_only_show_public_topics() {
    let dir = std::env::temp_dir().join(format!("forum-access-{}", Uuid::now_v7().simple()));
    std::fs::create_dir_all(dir.join("f")).unwrap();
    std::fs::write(
        dir.join("f/index.html"),
        "<html><head></head><body></body></html>",
    )
    .unwrap();
    std::env::set_var("WEB_ROOT", &dir);
    let (app, st) = app().await;
    let (author, _) = person(&st.db).await;
    let hidden = forum(&st.db, author, "list").await;
    let public = forum(&st.db, author, "public").await;

    let (_, html) = call(
        &app,
        "GET",
        &format!("/f/topico/{}", hidden.topic),
        None,
        None,
    )
    .await;
    assert!(!html.contains("sigiloso"), "{html}");
    let (_, html) = call(
        &app,
        "GET",
        &format!("/f/topico/{}", public.topic),
        None,
        None,
    )
    .await;
    assert!(html.contains(&public.title), "{html}");
}
