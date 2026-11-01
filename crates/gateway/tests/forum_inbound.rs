//! Contributions from the fediverse, against a real database and the real HTTP routes for what
//! readers see. HTTP signatures are verified before this code and are covered by http_surface.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_db::Db;
use dsoc_gateway::forum_inbound::{apply, InboundConfig};
use serde_json::{json, Value};
use tower::util::ServiceExt;
use uuid::Uuid;

const ORG: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
const HOST: &str = "agora.test";
const PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

async fn state() -> AppState {
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
    st
}

fn api(st: &AppState) -> Router {
    dsoc_gateway::api_router(st.clone())
}

/// A fresh forum; returns (id, handle).
async fn forum(db: &Db, visibility: &str, federated: bool) -> (Uuid, String) {
    let id = Uuid::now_v7();
    let slug = format!("f{}", id.simple());
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, federated, visibility)
         VALUES ($1, $2, $3, $3, $3, 'comunitario', $4, $5)",
    )
    .bind(id)
    .bind(ORG)
    .bind(&slug)
    .bind(federated)
    .bind(visibility)
    .execute(db)
    .await
    .expect("seed forum");
    (id, slug)
}

/// A citizen with a live session; returns (id, Cookie header).
async fn person(db: &Db, role: Option<&str>) -> (Uuid, String) {
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
    if let Some(role) = role {
        sqlx::query(
            "INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
             VALUES ($1, $2, $3, $4, now())",
        )
        .bind(Uuid::now_v7())
        .bind(ORG)
        .bind(citizen)
        .bind(role)
        .execute(db)
        .await
        .expect("seed role");
    }
    (citizen, format!("dsoc_session={session}"))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, HOST)
        .header(header::ACCEPT, "application/activity+json");
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, cookie);
    }
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let req = req
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn on() -> InboundConfig {
    InboundConfig {
        enabled: true,
        trusted_hosts: vec!["trusted.example".to_owned()],
        ..InboundConfig::default()
    }
}

fn group(handle: &str) -> String {
    format!("https://{HOST}/actors/{handle}")
}

fn unique_actor(host: &str) -> String {
    format!("https://{host}/u/{}", Uuid::now_v7().simple())
}

/// A Lemmy `Create(Page)`, shape from join-lemmy.org's federation documentation.
fn lemmy_post(actor: &str, handle: &str, title: &str) -> (Value, String) {
    let host = actor.split('/').nth(2).unwrap();
    let object_id = format!("https://{host}/post/{}", Uuid::now_v7().simple());
    let g = group(handle);
    (
        json!({
            "actor": actor,
            "to": [PUBLIC],
            "object": {
                "type": "Page",
                "id": object_id,
                "attributedTo": actor,
                "to": [g, PUBLIC],
                "audience": g,
                "name": title,
                "content": "<p>corpo <script>alert(1)</script></p>",
                "mediaType": "text/html",
                "published": "2021-10-29T15:10:51.557399Z"
            },
            "cc": [g],
            "audience": g,
            "type": "Create",
            "id": format!("https://{host}/activities/create/{}", Uuid::now_v7().simple())
        }),
        object_id,
    )
}

/// A Mastodon reply, shape of its `Create(Note)`.
fn mastodon_reply(actor: &str, parent: &str, text: &str) -> (Value, String) {
    let host = actor.split('/').nth(2).unwrap();
    let object_id = format!("https://{host}/statuses/{}", Uuid::now_v7().simple());
    (
        json!({
            "id": format!("{object_id}/activity"),
            "type": "Create",
            "actor": actor,
            "to": [PUBLIC],
            "cc": [format!("{actor}/followers")],
            "object": {
                "id": object_id,
                "type": "Note",
                "inReplyTo": parent,
                "attributedTo": actor,
                "to": [PUBLIC],
                "cc": [format!("{actor}/followers")],
                "content": format!("<p>{text}</p>"),
            }
        }),
        object_id,
    )
}

async fn send(st: &AppState, config: &InboundConfig, handle: &str, activity: &Value) -> StatusCode {
    apply(&st.db, config, HOST, handle, activity, Some("ana"))
        .await
        .expect("apply")
}

async fn topic_by_uri(db: &Db, uri: &str) -> Option<(Uuid, String, bool)> {
    sqlx::query_as(
        "SELECT id, moderation, hidden_at IS NOT NULL FROM forum_topic WHERE ap_object_uri = $1",
    )
    .bind(uri)
    .fetch_optional(db)
    .await
    .unwrap()
}

async fn relayed(db: &Db, forum: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM forum_outbound_activity WHERE forum_id = $1")
        .bind(forum)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn local_topic(db: &Db, forum: Uuid, author: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
         VALUES ($1, $2, $3, 'local', 'texto local', now())",
    )
    .bind(id)
    .bind(forum)
    .bind(author)
    .execute(db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn off_by_default_nothing_is_stored() {
    let st = state().await;
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (activity, uri) = lemmy_post(&unique_actor("trusted.example"), &handle, "t");
    let status = send(&st, &InboundConfig::default(), &handle, &activity).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(topic_by_uri(&st.db, &uri).await.is_none());
    assert_eq!(relayed(&st.db, forum_id).await, 0);
}

#[tokio::test]
async fn a_trusted_post_becomes_a_visible_topic_and_is_repeated() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (activity, uri) = lemmy_post(&unique_actor("trusted.example"), &handle, "Título");
    send(&st, &on(), &handle, &activity).await;
    let (id, moderation, hidden) = topic_by_uri(&st.db, &uri).await.unwrap();
    assert_eq!((moderation.as_str(), hidden), ("approved", false));

    let (status, body) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Título"));
    assert!(body.contains("@ana@trusted.example"), "{body}");
    assert!(!body.contains("alert(1)"), "{body}");
    // Wrapped for Lemmy and plain for Mastodon.
    assert_eq!(relayed(&st.db, forum_id).await, 2);
    let wrapped: Value = sqlx::query_scalar(
        "SELECT activity FROM forum_outbound_activity
          WHERE forum_id = $1 AND dedupe_key LIKE 'relay:%'",
    )
    .bind(forum_id)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(wrapped["type"], "Announce");
    assert_eq!(wrapped["actor"], group(&handle));
    assert_eq!(wrapped["object"]["id"], activity["id"]);
    assert_eq!(wrapped["object"]["object"]["id"], activity["object"]["id"]);

    // The same activity again, or a new one with the same object, adds nothing.
    send(&st, &on(), &handle, &activity).await;
    let copies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM forum_topic WHERE ap_object_uri = $1")
            .bind(&uri)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(copies, 1);
}

#[tokio::test]
async fn an_untrusted_post_waits_hidden_until_an_admin_approves() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    // Unique: the database outlives a test run and earlier runs approved theirs.
    let title = format!("Na fila {}", Uuid::now_v7().simple());
    let (activity, uri) = lemmy_post(&unique_actor("other.example"), &handle, &title);
    send(&st, &on(), &handle, &activity).await;
    let (id, moderation, hidden) = topic_by_uri(&st.db, &uri).await.unwrap();
    assert_eq!((moderation.as_str(), hidden), ("pending", true));
    assert_eq!(relayed(&st.db, forum_id).await, 0);

    let (member_id, member) = person(&st.db, None).await;
    let _ = member_id;
    for (cookie, uri) in [
        (None, format!("/api/v1/f/topics/{id}")),
        (Some(member.as_str()), format!("/api/v1/f/topics/{id}")),
    ] {
        let (status, body) = call(&app, "GET", &uri, cookie, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let (_, list) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics?path={handle}"),
        None,
        None,
    )
    .await;
    assert!(!list.contains(&title), "{list}");
    let (_, recent) = call(&app, "GET", "/api/v1/f/recent?limit=50", None, None).await;
    assert!(!recent.contains(&title), "{recent}");
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/f/topics/{id}/vote"),
        Some(&member),
        Some(json!({"stance": "favor"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let approve = format!("/api/v1/admin/forums/inbound/queue/topic/{id}/approve");
    let (status, _) = call(
        &app,
        "GET",
        "/api/v1/admin/forums/inbound/queue",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&app, "POST", &approve, Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(topic_by_uri(&st.db, &uri).await.unwrap().1, "pending");

    let (_, admin) = person(&st.db, Some("admin")).await;
    let (status, queue) = call(
        &app,
        "GET",
        "/api/v1/admin/forums/inbound/queue",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(queue.contains(&title), "{queue}");
    let (status, body) = call(&app, "POST", &approve, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    // Decided once.
    let (status, _) = call(&app, "POST", &approve, Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_rejected_post_stays_hidden() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let (activity, uri) = lemmy_post(&unique_actor("other.example"), &handle, "Spam");
    send(&st, &on(), &handle, &activity).await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;
    let (_, admin) = person(&st.db, Some("admin")).await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/admin/forums/inbound/queue/topic/{id}/reject"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_restricted_or_unfederated_forum_takes_nothing() {
    let st = state().await;
    for (visibility, federated) in [("members", false), ("list", false), ("public", false)] {
        let (forum_id, handle) = forum(&st.db, visibility, federated).await;
        let (activity, uri) = lemmy_post(&unique_actor("trusted.example"), &handle, "t");
        let status = send(&st, &on(), &handle, &activity).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(topic_by_uri(&st.db, &uri).await.is_none());
        assert_eq!(relayed(&st.db, forum_id).await, 0);
    }
}

#[tokio::test]
async fn a_post_not_addressed_to_this_forum_or_not_public_is_ignored() {
    let st = state().await;
    let (_, handle) = forum(&st.db, "public", true).await;
    let (_, other) = forum(&st.db, "public", true).await;
    let (activity, uri) = lemmy_post(&unique_actor("trusted.example"), &other, "t");
    send(&st, &on(), &handle, &activity).await;
    assert!(topic_by_uri(&st.db, &uri).await.is_none());

    let (mut direct, uri) = lemmy_post(&unique_actor("trusted.example"), &handle, "dm");
    direct["object"]["to"] = json!([group(&handle)]);
    send(&st, &on(), &handle, &direct).await;
    assert!(topic_by_uri(&st.db, &uri).await.is_none());
}

#[tokio::test]
async fn replies_become_comments_only_on_this_forums_visible_topics() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let parent = format!("{}/objects/{topic}", group(&handle));
    let actor = unique_actor("trusted.example");

    let (reply, reply_uri) = mastodon_reply(&actor, &parent, "resposta federada");
    send(&st, &on(), &handle, &reply).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(body.contains("resposta federada"), "{body}");
    assert!(body.contains("@ana@trusted.example"), "{body}");

    // A reply to that reply lands on the same topic.
    let (nested, _) = mastodon_reply(&actor, &reply_uri, "resposta aninhada");
    send(&st, &on(), &handle, &nested).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(body.contains("resposta aninhada"), "{body}");

    // From an untrusted server it waits, and federated comments never count for thresholds.
    let (queued, _) = mastodon_reply(&unique_actor("other.example"), &parent, "na fila");
    send(&st, &on(), &handle, &queued).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(!body.contains("na fila"), "{body}");
    let counters: (i64, i64) = sqlx::query_as(
        "SELECT interaction_count, federated_interaction_count FROM forum_topic WHERE id = $1",
    )
    .bind(topic)
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(counters, (0, 2));

    // A parent elsewhere, including a topic of another forum, is not a reply here.
    let (other_forum, _) = forum(&st.db, "public", true).await;
    let elsewhere = local_topic(&st.db, other_forum, author).await;
    let (stray, stray_uri) = mastodon_reply(
        &actor,
        &format!("{}/objects/{elsewhere}", group(&handle)),
        "fora",
    );
    send(&st, &on(), &handle, &stray).await;
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM forum_topic_comment WHERE ap_object_uri = $1")
            .bind(&stray_uri)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn nobody_can_speak_for_or_change_someone_elses_post() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let owner = unique_actor("trusted.example");
    let intruder = unique_actor("trusted.example");
    let (activity, uri) = lemmy_post(&owner, &handle, "Original");
    send(&st, &on(), &handle, &activity).await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;

    let mut forged = lemmy_post(&intruder, &handle, "Forjado").0;
    forged["object"]["attributedTo"] = json!(owner);
    send(&st, &on(), &handle, &forged).await;
    let mut update = activity.clone();
    update["type"] = json!("Update");
    update["id"] = json!(format!("{intruder}/activities/1"));
    update["actor"] = json!(intruder);
    update["object"]["attributedTo"] = json!(intruder);
    update["object"]["name"] = json!("Trocado");
    send(&st, &on(), &handle, &update).await;
    let delete = json!({
        "id": format!("{intruder}/activities/2"), "type": "Delete", "actor": intruder,
        "to": [PUBLIC], "object": uri,
    });
    send(&st, &on(), &handle, &delete).await;

    let (status, body) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Original") && !body.contains("Trocado"),
        "{body}"
    );
    let (_, list) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics?path={handle}"),
        None,
        None,
    )
    .await;
    assert!(!list.contains("Forjado"), "{list}");
}

#[tokio::test]
async fn the_author_can_edit_and_delete() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let owner = unique_actor("trusted.example");
    let (activity, uri) = lemmy_post(&owner, &handle, "Antes");
    send(&st, &on(), &handle, &activity).await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;

    let mut update = activity.clone();
    update["type"] = json!("Update");
    update["id"] = json!(format!("{owner}/activities/edit"));
    update["object"]["name"] = json!("Depois");
    send(&st, &on(), &handle, &update).await;
    let (_, body) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert!(body.contains("Depois"), "{body}");

    let delete = json!({
        "id": format!("{owner}/activities/del"), "type": "Delete", "actor": owner,
        "to": [PUBLIC], "object": uri,
    });
    send(&st, &on(), &handle, &delete).await;
    let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_untrusted_edit_goes_back_to_the_queue() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let owner = unique_actor("other.example");
    let (activity, uri) = lemmy_post(&owner, &handle, "Aprovado");
    send(&st, &on(), &handle, &activity).await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;
    let (_, admin) = person(&st.db, Some("admin")).await;
    call(
        &app,
        "POST",
        &format!("/api/v1/admin/forums/inbound/queue/topic/{id}/approve"),
        Some(&admin),
        None,
    )
    .await;
    let mut update = activity.clone();
    update["type"] = json!("Update");
    update["id"] = json!(format!("{owner}/activities/edit"));
    update["object"]["name"] = json!("Virou spam");
    send(&st, &on(), &handle, &update).await;
    let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(topic_by_uri(&st.db, &uri).await.unwrap().1, "pending");
}

#[tokio::test]
async fn remote_votes_show_but_never_touch_score_or_thresholds() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let object = format!("{}/objects/{topic}", group(&handle));
    let counters = || async {
        sqlx::query_as::<_, (i64, i64, i64, i64, i64)>(
            "SELECT favor_count, contra_count, score, interaction_count, federated_interaction_count
               FROM forum_topic WHERE id = $1",
        )
        .bind(topic)
        .fetch_one(&st.db)
        .await
        .unwrap()
    };
    let liker = unique_actor("other.example");
    let like = json!({"id": format!("{liker}/like/1"), "type": "Like", "actor": liker,
                      "object": object, "audience": group(&handle)});
    send(&st, &on(), &handle, &like).await;
    let disliker = unique_actor("other.example");
    let dislike = json!({"id": format!("{disliker}/dislike/1"), "type": "Dislike",
                         "actor": disliker, "object": object});
    send(&st, &on(), &handle, &dislike).await;
    assert_eq!(counters().await, (1, 1, 0, 0, 2));

    // Voting twice is one vote, and changing it moves it.
    let again = json!({"id": format!("{liker}/dislike/2"), "type": "Dislike", "actor": liker,
                       "object": object});
    send(&st, &on(), &handle, &again).await;
    assert_eq!(counters().await, (0, 2, 0, 0, 2));

    // A local vote recounts and keeps the remote ones.
    let (_, member) = person(&st.db, None).await;
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v1/f/topics/{topic}/vote"),
        Some(&member),
        Some(json!({"stance": "favor"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(counters().await, (1, 2, 1, 1, 2));

    // Only the voter can undo a vote.
    let forged = json!({"id": format!("{disliker}/undo/9"), "type": "Undo", "actor": disliker,
                        "object": {"type": "Dislike", "actor": liker, "object": object}});
    send(&st, &on(), &handle, &forged).await;
    assert_eq!(counters().await, (1, 2, 1, 1, 2));
    let undo = json!({"id": format!("{liker}/undo/1"), "type": "Undo", "actor": liker,
                      "object": {"type": "Dislike", "actor": liker, "object": object}});
    send(&st, &on(), &handle, &undo).await;
    assert_eq!(counters().await, (1, 1, 1, 1, 1));
}

#[tokio::test]
async fn a_blocked_account_or_server_is_refused_and_its_queue_dropped() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let (_, admin) = person(&st.db, Some("admin")).await;
    let (_, member) = person(&st.db, None).await;
    let blocks = "/api/v1/admin/forums/inbound/blocks";
    let actor = unique_actor("other.example");
    let (queued, queued_uri) = lemmy_post(&actor, &handle, "antes do bloqueio");
    send(&st, &on(), &handle, &queued).await;

    let body = json!({"kind": "actor", "value": actor});
    let (status, _) = call(&app, "POST", blocks, Some(&member), Some(body.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, text) = call(&app, "POST", blocks, Some(&admin), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    assert_eq!(
        topic_by_uri(&st.db, &queued_uri).await.unwrap().1,
        "rejected"
    );
    let (activity, uri) = lemmy_post(&actor, &handle, "depois");
    assert_eq!(
        send(&st, &on(), &handle, &activity).await,
        StatusCode::FORBIDDEN
    );
    assert!(topic_by_uri(&st.db, &uri).await.is_none());

    let domain = format!("d{}.example", Uuid::now_v7().simple());
    let (status, _) = call(
        &app,
        "POST",
        blocks,
        Some(&admin),
        Some(json!({"kind": "domain", "value": domain})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (activity, uri) = lemmy_post(&unique_actor(&domain), &handle, "x");
    assert_eq!(
        send(&st, &on(), &handle, &activity).await,
        StatusCode::FORBIDDEN
    );
    assert!(topic_by_uri(&st.db, &uri).await.is_none());

    let (status, list) = call(&app, "GET", blocks, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list.contains(&domain) && list.contains(&actor), "{list}");
    let (status, _) = call(
        &app,
        "POST",
        blocks,
        Some(&admin),
        Some(json!({"kind": "domain", "value": "not a domain"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_remote_author_gets_no_account_that_can_sign_in() {
    let st = state().await;
    let (_, handle) = forum(&st.db, "public", true).await;
    let actor = unique_actor("trusted.example");
    let (activity, _) = lemmy_post(&actor, &handle, "t");
    send(&st, &on(), &handle, &activity).await;
    let mine: i64 = sqlx::query_scalar("SELECT count(*) FROM citizen WHERE oidc_subject = $1")
        .bind(&actor)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(mine, 0);
    let credentials: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_credential a JOIN citizen c ON c.id = a.citizen_id
          WHERE c.oidc_subject = 'system:fediverse'",
    )
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(credentials, 0);
}

#[tokio::test]
async fn a_private_author_resolves_without_exposing_the_person() {
    let st = state().await;
    let public = dsoc_gateway::federation::public_routes(st.clone());
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    sqlx::query("UPDATE citizen SET handle = $2, display_name = 'Nome Real', is_public = false WHERE id = $1")
        .bind(author)
        .bind(format!("h{}", &author.simple().to_string()[20..]))
        .execute(&st.db)
        .await
        .unwrap();
    let opaque = format!("u_{}", author.simple());

    // Before posting anything the actor does not exist.
    let (status, _) = call(&public, "GET", &format!("/actors/{opaque}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let topic = local_topic(&st.db, forum_id, author).await;
    let (status, page) = call(
        &public,
        "GET",
        &format!("/actors/{handle}/objects/{topic}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page: Value = serde_json::from_str(&page).unwrap();
    assert_eq!(page["type"], "Page");
    assert_eq!(page["name"], "local");
    assert_eq!(
        page["attributedTo"],
        format!("https://{HOST}/actors/{opaque}")
    );

    let (status, actor) = call(&public, "GET", &format!("/actors/{opaque}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "{actor}");
    assert!(!actor.contains("Nome Real"), "{actor}");
    let actor: Value = serde_json::from_str(&actor).unwrap();
    assert_eq!(actor["type"], "Person");
    assert!(actor["publicKey"]["publicKeyPem"].is_string());
    let (status, _) = call(
        &public,
        "GET",
        &format!("/.well-known/webfinger?resource=acct:{opaque}@{HOST}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_group_serves_what_lemmy_fetches() {
    let st = state().await;
    let public = dsoc_gateway::federation::public_routes(st.clone());
    let (_, handle) = forum(&st.db, "public", true).await;
    let (status, doc) = call(&public, "GET", &format!("/actors/{handle}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let doc: Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(doc["type"], "Group");
    assert!(doc.get("icon").is_none());
    for path in ["followers", "outbox"] {
        let (status, body) = call(
            &public,
            "GET",
            &format!("/actors/{handle}/{path}"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(body.contains("OrderedCollection"), "{body}");
    }
}

#[tokio::test]
async fn the_sweep_sends_local_comments_only_when_opted_in_and_never_from_a_restricted_forum() {
    let st = state().await;
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let comment = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO forum_topic_comment (id, topic_id, author_id, body, created_at)
         VALUES ($1, $2, $3, 'argumento', now())",
    )
    .bind(comment)
    .bind(topic)
    .bind(author)
    .execute(&st.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO forum_follower (forum_id, remote_actor_url, remote_inbox_url, accepted_at, created_at)
         VALUES ($1, 'https://test.invalid/u/a', 'https://test.invalid/inbox', now(), now() - interval '1 hour')",
    )
    .bind(forum_id)
    .execute(&st.db)
    .await
    .unwrap();

    dsoc_gateway::forum_outbound::sweep(&st.db, HOST, &InboundConfig::default()).await;
    assert_eq!(relayed(&st.db, forum_id).await, 0);

    dsoc_gateway::forum_outbound::sweep(&st.db, HOST, &on()).await;
    let activity: Value =
        sqlx::query_scalar("SELECT activity FROM forum_outbound_activity WHERE forum_id = $1")
            .bind(forum_id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(activity["object"]["type"], "Create");
    assert_eq!(activity["object"]["object"]["type"], "Note");
    assert_eq!(
        activity["object"]["object"]["inReplyTo"],
        format!("{}/objects/{topic}", group(&handle))
    );
    let attempts = || async {
        sqlx::query_scalar::<_, i32>(
            "SELECT d.attempts FROM forum_outbound_delivery d
               JOIN forum_outbound_activity a ON a.id = d.activity_id WHERE a.forum_id = $1",
        )
        .bind(forum_id)
        .fetch_one(&st.db)
        .await
        .unwrap()
    };
    // The test host does not resolve, so the delivery was tried and counted.
    assert_eq!(attempts().await, 1);

    sqlx::query("UPDATE forum SET federated = false, visibility = 'list' WHERE id = $1")
        .bind(forum_id)
        .execute(&st.db)
        .await
        .unwrap();
    dsoc_gateway::forum_outbound::sweep(&st.db, HOST, &on()).await;
    assert_eq!(attempts().await, 1);
}

#[tokio::test]
async fn a_removal_is_announced_for_public_forums_only() {
    let st = state().await;
    let (forum_id, _) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    dsoc_gateway::forum_outbound::enqueue_removal(&st.db, HOST, "topic", topic, Some("spam"))
        .await
        .unwrap();
    let activity: Value =
        sqlx::query_scalar("SELECT activity FROM forum_outbound_activity WHERE forum_id = $1")
            .bind(forum_id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(activity["object"]["type"], "Delete");
    assert_eq!(activity["object"]["summary"], "spam");
    assert_eq!(
        activity["object"]["actor"],
        format!("https://{HOST}/actors/u_{}", author.simple())
    );

    let (private, _) = forum(&st.db, "list", false).await;
    let hidden = local_topic(&st.db, private, author).await;
    dsoc_gateway::forum_outbound::enqueue_removal(&st.db, HOST, "topic", hidden, None)
        .await
        .unwrap();
    assert_eq!(relayed(&st.db, private).await, 0);
}

#[tokio::test]
async fn a_comment_can_be_edited_and_deleted_by_its_author() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let parent = format!("{}/objects/{topic}", group(&handle));
    let owner = unique_actor("trusted.example");
    let (reply, uri) = mastodon_reply(&owner, &parent, "primeira versao");
    send(&st, &on(), &handle, &reply).await;

    let mut update = reply.clone();
    update["type"] = json!("Update");
    update["id"] = json!(format!("{owner}/activities/edit"));
    update["object"]["content"] = json!("<p>segunda versao</p>");
    send(&st, &on(), &handle, &update).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(
        body.contains("segunda versao") && !body.contains("primeira"),
        "{body}"
    );

    let delete = json!({
        "id": format!("{owner}/activities/del"), "type": "Delete", "actor": owner,
        "to": [PUBLIC], "object": {"id": uri, "type": "Tombstone"},
    });
    send(&st, &on(), &handle, &delete).await;
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(!body.contains("segunda versao"), "{body}");
}

#[tokio::test]
async fn a_deleted_remote_account_takes_its_content_and_votes() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let object = format!("{}/objects/{topic}", group(&handle));
    let gone = unique_actor("trusted.example");
    let (post, uri) = lemmy_post(&gone, &handle, "Do que saiu");
    send(&st, &on(), &handle, &post).await;
    let like = json!({"id": format!("{gone}/l"), "type": "Like", "actor": gone, "object": object});
    send(&st, &on(), &handle, &like).await;

    let erase = json!({"id": format!("{gone}#delete"), "type": "Delete", "actor": gone,
                       "to": [PUBLIC], "object": gone});
    send(&st, &on(), &handle, &erase).await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;
    let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let favor: i64 = sqlx::query_scalar("SELECT favor_count FROM forum_topic WHERE id = $1")
        .bind(topic)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(favor, 0);
}

// --- Security review of #88 ---

async fn queue_body(app: &Router, admin: &str) -> String {
    call(
        app,
        "GET",
        "/api/v1/admin/forums/inbound/queue",
        Some(admin),
        None,
    )
    .await
    .1
}

fn update_of(activity: &Value, actor: &str, name: &str, content: &str) -> Value {
    let mut update = activity.clone();
    update["type"] = json!("Update");
    update["id"] = json!(format!("{actor}/activities/{}", Uuid::now_v7().simple()));
    update["object"]["name"] = json!(name);
    update["object"]["content"] = json!(content);
    update
}

async fn relays(db: &Db, forum: Uuid) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT activity FROM forum_outbound_activity WHERE forum_id = $1 ORDER BY id",
    )
    .bind(forum)
    .fetch_all(db)
    .await
    .unwrap()
}

#[tokio::test]
async fn another_port_on_a_trusted_host_is_not_that_host() {
    let st = state().await;
    let (_, handle) = forum(&st.db, "public", true).await;
    let (activity, uri) = lemmy_post(
        &format!("https://trusted.example:8443/u/{}", Uuid::now_v7().simple()),
        &handle,
        "porta",
    );
    send(&st, &on(), &handle, &activity).await;
    assert!(topic_by_uri(&st.db, &uri).await.is_none());
}

#[tokio::test]
async fn a_non_canonical_actor_is_ignored() {
    let st = state().await;
    let (_, handle) = forum(&st.db, "public", true).await;
    for actor in [
        "https://other.example/u/a?x=1",
        "https://me@other.example/u/a",
        "https://OTHER.example/u/a",
        "https://other.example./u/a",
    ] {
        let (mut activity, uri) = lemmy_post("https://other.example/u/a", &handle, "x");
        activity["actor"] = json!(actor);
        activity["object"]["attributedTo"] = json!(actor);
        send(&st, &on(), &handle, &activity).await;
        assert!(topic_by_uri(&st.db, &uri).await.is_none(), "{actor}");
    }
}

#[tokio::test]
async fn the_queue_is_capped_per_host_and_per_forum() {
    let st = state().await;
    let (_, handle) = forum(&st.db, "public", true).await;
    let host = format!("h{}.example", Uuid::now_v7().simple());
    let capped = InboundConfig {
        max_pending_per_host: 2,
        ..on()
    };
    let mut stored = 0;
    for _ in 0..4 {
        let (activity, uri) = lemmy_post(&unique_actor(&host), &handle, "fila");
        send(&st, &capped, &handle, &activity).await;
        stored += i32::from(topic_by_uri(&st.db, &uri).await.is_some());
    }
    assert_eq!(stored, 2);

    let (_, small) = forum(&st.db, "public", true).await;
    let capped = InboundConfig {
        max_pending_per_forum: 1,
        ..on()
    };
    let mut stored = 0;
    for _ in 0..3 {
        let flood = format!("h{}.example", Uuid::now_v7().simple());
        let (activity, uri) = lemmy_post(&unique_actor(&flood), &small, "fila");
        send(&st, &capped, &small, &activity).await;
        stored += i32::from(topic_by_uri(&st.db, &uri).await.is_some());
    }
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn one_flooding_server_cannot_bury_another_in_the_queue() {
    let st = state().await;
    let app = api(&st);
    let (_, handle) = forum(&st.db, "public", true).await;
    let flood = format!("h{}.example", Uuid::now_v7().simple());
    let wide = InboundConfig {
        max_pending_per_host: 1000,
        max_pending_per_forum: 1000,
        ..on()
    };
    for _ in 0..205 {
        let (activity, _) = lemmy_post(&unique_actor(&flood), &handle, "flood");
        send(&st, &wide, &handle, &activity).await;
    }
    let title = format!("legitimo {}", Uuid::now_v7().simple());
    let (activity, _) = lemmy_post(&unique_actor("late.example"), &handle, &title);
    send(&st, &wide, &handle, &activity).await;
    let (_, admin) = person(&st.db, Some("admin")).await;
    assert!(queue_body(&app, &admin).await.contains(&title));
}

/// A forum moderator's session.
async fn moderator(db: &Db, forum: Uuid) -> String {
    let (citizen, cookie) = person(db, None).await;
    sqlx::query("INSERT INTO forum_moderator (forum_id, citizen_id) VALUES ($1, $2)")
        .bind(forum)
        .bind(citizen)
        .execute(db)
        .await
        .unwrap();
    cookie
}

async fn moderator_removes(app: &Router, admin: &str, kind: &str, id: Uuid) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/f/{kind}/{id}/remove"),
        Some(admin),
        Some(json!({"reason": "regra 3"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_moderator_removal_of_a_remote_topic_is_final() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let admin = moderator(&st.db, forum_id).await;
    for owner in [
        unique_actor("trusted.example"),
        unique_actor("other.example"),
    ] {
        let (activity, uri) = lemmy_post(&owner, &handle, "Removido");
        send(&st, &on(), &handle, &activity).await;
        let id = topic_by_uri(&st.db, &uri).await.unwrap().0;
        dsoc_gateway::forum_inbound_admin::decide_item(&st.db, Some(HOST), ORG, "topic", id, true)
            .await
            .unwrap();
        moderator_removes(&app, &admin, "topics", id).await;
        let before = relays(&st.db, forum_id).await.len();

        send(
            &st,
            &on(),
            &handle,
            &update_of(&activity, &owner, "Voltei", "<p>x</p>"),
        )
        .await;
        let (title, moderation): (String, String) =
            sqlx::query_as("SELECT title, moderation FROM forum_topic WHERE id = $1")
                .bind(id)
                .fetch_one(&st.db)
                .await
                .unwrap();
        assert_eq!(title, "Removido");
        assert_ne!(moderation, "pending");
        let approved = dsoc_gateway::forum_inbound_admin::decide_item(
            &st.db,
            Some(HOST),
            ORG,
            "topic",
            id,
            true,
        )
        .await
        .unwrap();
        assert!(!approved);
        let delete = json!({"id": format!("{owner}/d/{id}"), "type": "Delete", "actor": owner,
                            "to": [PUBLIC], "object": uri});
        send(&st, &on(), &handle, &delete).await;
        let reason: Option<String> =
            sqlx::query_scalar("SELECT deletion_reason FROM forum_topic WHERE id = $1")
                .bind(id)
                .fetch_one(&st.db)
                .await
                .unwrap();
        assert_eq!(reason.as_deref(), Some("regra 3"));
        let (status, _) = call(&app, "GET", &format!("/api/v1/f/topics/{id}"), None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(relays(&st.db, forum_id).await.len(), before);
    }
}

#[tokio::test]
async fn a_moderator_removal_of_a_remote_comment_is_final() {
    let st = state().await;
    let app = api(&st);
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (author, _) = person(&st.db, None).await;
    let topic = local_topic(&st.db, forum_id, author).await;
    let parent = format!("{}/objects/{topic}", group(&handle));
    let admin = moderator(&st.db, forum_id).await;
    let owner = unique_actor("trusted.example");
    let (reply, uri) = mastodon_reply(&owner, &parent, "comentario removido");
    send(&st, &on(), &handle, &reply).await;
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM forum_topic_comment WHERE ap_object_uri = $1")
            .bind(&uri)
            .fetch_one(&st.db)
            .await
            .unwrap();
    moderator_removes(&app, &admin, "comments", id).await;

    let mut edit = reply.clone();
    edit["type"] = json!("Update");
    edit["id"] = json!(format!("{owner}/edit/1"));
    edit["object"]["content"] = json!("<p>voltei</p>");
    send(&st, &on(), &handle, &edit).await;
    send(
        &st,
        &InboundConfig {
            trusted_hosts: vec![],
            ..on()
        },
        &handle,
        &edit,
    )
    .await;
    let approved = dsoc_gateway::forum_inbound_admin::decide_item(
        &st.db,
        Some(HOST),
        ORG,
        "comment",
        id,
        true,
    )
    .await
    .unwrap();
    assert!(!approved);
    let delete = json!({"id": format!("{owner}/d/1"), "type": "Delete", "actor": owner,
                        "to": [PUBLIC], "object": uri});
    send(&st, &on(), &handle, &delete).await;
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT body, deletion_reason FROM forum_topic_comment WHERE id = $1")
            .bind(id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(row.0, "comentario removido");
    assert_eq!(row.1.as_deref(), Some("regra 3"));
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/f/topics/{topic}"),
        None,
        None,
    )
    .await;
    assert!(!body.contains("comentario removido"), "{body}");
}

#[tokio::test]
async fn an_edit_of_something_not_public_is_not_relayed() {
    let st = state().await;
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let host = format!("h{}.example", Uuid::now_v7().simple());
    let owner = unique_actor(&host);
    let (activity, _) = lemmy_post(&owner, &handle, "Na fila");
    send(&st, &on(), &handle, &activity).await;
    // The host became trusted while the post waits: the edit applies, the post is still pending.
    let trusted_now = InboundConfig {
        trusted_hosts: vec![host],
        ..on()
    };
    send(
        &st,
        &trusted_now,
        &handle,
        &update_of(&activity, &owner, "Editado", "<p>x</p>"),
    )
    .await;
    assert!(relays(&st.db, forum_id).await.is_empty());
}

#[tokio::test]
async fn what_is_relayed_is_rebuilt_from_what_was_stored() {
    let st = state().await;
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let (mut activity, _) = lemmy_post(&unique_actor("trusted.example"), &handle, "Limpo");
    activity["object"]["content"] = json!("<p>texto</p><script>mal()</script><img src=x>");
    activity["object"]["attachment"] =
        json!([{"type": "Document", "url": "https://x.example/mal.exe", "name": "mal"}]);
    activity["hidden_payload"] = json!("nao visto");
    activity["object"]["hidden_payload"] = json!("nao visto");
    send(&st, &on(), &handle, &activity).await;
    let wrapped = relays(&st.db, forum_id)
        .await
        .into_iter()
        .find(|a| a["object"].is_object())
        .unwrap();
    let text = wrapped.to_string();
    assert!(!text.contains("nao visto"), "{text}");
    assert!(!text.contains("<script"), "{text}");
    assert!(!text.contains("<img"), "{text}");
    assert_eq!(wrapped["object"]["type"], "Create");
    assert_eq!(wrapped["object"]["object"]["name"], "Limpo");
    assert_eq!(wrapped["object"]["object"]["id"], activity["object"]["id"]);
}

#[tokio::test]
async fn approving_an_edited_item_never_relayed_sends_a_create() {
    let st = state().await;
    let (forum_id, handle) = forum(&st.db, "public", true).await;
    let owner = unique_actor("other.example");
    let (activity, uri) = lemmy_post(&owner, &handle, "Primeira");
    send(&st, &on(), &handle, &activity).await;
    send(
        &st,
        &on(),
        &handle,
        &update_of(&activity, &owner, "Segunda", "<p>editado</p>"),
    )
    .await;
    let id = topic_by_uri(&st.db, &uri).await.unwrap().0;
    assert!(dsoc_gateway::forum_inbound_admin::decide_item(
        &st.db,
        Some(HOST),
        ORG,
        "topic",
        id,
        true
    )
    .await
    .unwrap());
    let wrapped = relays(&st.db, forum_id)
        .await
        .into_iter()
        .find(|a| a["object"].is_object())
        .unwrap();
    assert_eq!(wrapped["object"]["type"], "Create");
    assert_eq!(wrapped["object"]["object"]["name"], "Segunda");

    // Once it went out, a later approved edit is an Update.
    send(
        &st,
        &on(),
        &handle,
        &update_of(&activity, &owner, "Terceira", "<p>de novo</p>"),
    )
    .await;
    assert!(dsoc_gateway::forum_inbound_admin::decide_item(
        &st.db,
        Some(HOST),
        ORG,
        "topic",
        id,
        true
    )
    .await
    .unwrap());
    let last = relays(&st.db, forum_id).await.pop().unwrap();
    assert_eq!(last["object"]["type"], "Update");
    assert_eq!(last["object"]["object"]["name"], "Terceira");
}
