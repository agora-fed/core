//! Real HTTP Signatures through the forum inbox: digest, date and RSA signature are produced and
//! verified for real. Only the fetch of the signer's document is answered locally, because the SSRF
//! guard refuses to reach a test server.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, LazyLock};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use base64::Engine as _;
use dsoc_app::AppState;
use dsoc_core::{Clock, SystemClock};
use dsoc_federation::{build_signing_string, generate_actor_keypair, GeneratedKeypair};
use dsoc_gateway::forum_inbound::InboundConfig;
use serde_json::{json, Value};
use sha2::Digest as _;
use tower::util::ServiceExt;
use uuid::Uuid;

const ORG: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
const HOST: &str = "agora.test";
const PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

static KEY: LazyLock<GeneratedKeypair> = LazyLock::new(|| generate_actor_keypair().unwrap());

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

async fn forum(st: &AppState) -> String {
    let id = Uuid::now_v7();
    let slug = format!("f{}", id.simple());
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, federated, visibility)
         VALUES ($1, $2, $3, $3, $3, 'comunitario', true, 'public')",
    )
    .bind(id)
    .bind(ORG)
    .bind(&slug)
    .execute(&st.db)
    .await
    .expect("seed forum");
    slug
}

/// An inbox whose actor fetch serves a document for the requested URL, with `doc_id` as its id
/// (the URL itself when `None`).
fn inbox(st: &AppState, config: InboundConfig, doc_id: Option<String>) -> Router {
    Router::new()
        .route(
            "/actors/{handle}/inbox",
            post(
                move |State(state): State<AppState>,
                      headers: HeaderMap,
                      Path(handle): Path<String>,
                      body: Bytes| {
                    let config = config.clone();
                    let doc_id = doc_id.clone();
                    async move {
                        let r: Response = dsoc_gateway::federation::process_inbox(
                            state,
                            headers,
                            handle,
                            body,
                            config,
                            move |url: String| async move {
                                let id = doc_id.unwrap_or_else(|| url.clone());
                                Ok(json!({
                                    "id": id,
                                    "type": "Person",
                                    "preferredUsername": "ana",
                                    "inbox": format!("{url}/inbox"),
                                    "publicKey": {
                                        "id": format!("{url}#main-key"),
                                        "owner": url,
                                        "publicKeyPem": KEY.public_pem,
                                    }
                                }))
                            },
                        )
                        .await;
                        r
                    }
                },
            ),
        )
        .with_state(st.clone())
}

fn create(actor: &str, handle: &str) -> (Value, String) {
    let host = actor.split('/').nth(2).unwrap();
    let object = format!("https://{host}/post/{}", Uuid::now_v7().simple());
    let g = format!("https://{HOST}/actors/{handle}");
    (
        json!({
            "id": format!("https://{host}/activities/{}", Uuid::now_v7().simple()),
            "type": "Create",
            "actor": actor,
            "to": [PUBLIC],
            "cc": [g],
            "object": {
                "id": object, "type": "Page", "attributedTo": actor,
                "to": [g, PUBLIC], "audience": g, "name": "assinado",
                "content": "<p>corpo</p>",
            }
        }),
        object,
    )
}

async fn post_signed(app: &Router, handle: &str, actor: &str, activity: &Value) -> StatusCode {
    post_signed_with(
        app,
        handle,
        &format!("{actor}#main-key"),
        activity,
        &KEY.private_pem,
    )
    .await
}

async fn post_signed_with(
    app: &Router,
    handle: &str,
    key_id: &str,
    activity: &Value,
    private_pem: &str,
) -> StatusCode {
    let body = activity.to_string();
    let digest = format!(
        "SHA-256={}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body.as_bytes()))
    );
    let date = chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let target = format!("/actors/{handle}/inbox");
    let covered = ["(request-target)", "host", "date", "digest"];
    let pairs = vec![
        ("host".to_owned(), HOST.to_owned()),
        ("date".to_owned(), date.clone()),
        ("digest".to_owned(), digest.clone()),
    ];
    let signing = build_signing_string("post", &target, &pairs, &covered).unwrap();
    let signature = dsoc_federation::sign_with_pem(private_pem, &signing).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(&target)
        .header(header::HOST, HOST)
        .header(header::DATE, date)
        .header("digest", digest)
        .header(
            "signature",
            dsoc_federation::signature_header_value(key_id, &covered, &signature),
        )
        .header(header::CONTENT_TYPE, "application/activity+json")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let _ = to_bytes(res.into_body(), 1 << 20).await;
    status
}

async fn stored(st: &AppState, uri: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM forum_topic WHERE ap_object_uri = $1)")
        .bind(uri)
        .fetch_one(&st.db)
        .await
        .unwrap()
}

async fn seen(st: &AppState, activity: &Value) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM forum_inbox_seen WHERE activity_id = $1)")
        .bind(activity["id"].as_str().unwrap())
        .fetch_one(&st.db)
        .await
        .unwrap()
}

fn fresh_host() -> String {
    format!("h{}.example", Uuid::now_v7().simple())
}

fn fresh_actor(host: &str) -> String {
    format!("https://{host}/u/{}", Uuid::now_v7().simple())
}

fn on() -> InboundConfig {
    InboundConfig {
        enabled: true,
        ..InboundConfig::default()
    }
}

#[tokio::test]
async fn a_signed_create_becomes_a_queued_topic() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(&st, on(), None);
    let actor = fresh_actor(&fresh_host());
    let (activity, uri) = create(&actor, &handle);
    assert_eq!(
        post_signed(&app, &handle, &actor, &activity).await,
        StatusCode::ACCEPTED
    );
    assert!(stored(&st, &uri).await);
    let handle_stored: Option<String> =
        sqlx::query_scalar("SELECT remote_handle FROM forum_topic WHERE ap_object_uri = $1")
            .bind(&uri)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert!(handle_stored.unwrap().starts_with("@ana@"));
    // Delivered twice, it is one topic.
    assert_eq!(
        post_signed(&app, &handle, &actor, &activity).await,
        StatusCode::ACCEPTED
    );
}

#[tokio::test]
async fn a_signature_from_another_key_or_another_actor_is_refused() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(&st, on(), None);
    let actor = fresh_actor(&fresh_host());
    let (activity, uri) = create(&actor, &handle);
    let other = generate_actor_keypair().unwrap();
    let status = post_signed_with(
        &app,
        &handle,
        &format!("{actor}#main-key"),
        &activity,
        &other.private_pem,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let impostor = fresh_actor(&fresh_host());
    assert_eq!(
        post_signed(&app, &handle, &impostor, &activity).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!stored(&st, &uri).await);
}

#[tokio::test]
async fn a_non_canonical_signer_is_refused() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(&st, on(), None);
    let host = fresh_host();
    let id = Uuid::now_v7().simple();
    for actor in [
        format!("https://{host}/u/{id}?x=1"),
        format!("https://me@{host}/u/{id}"),
        format!("https://{}/u/{id}", host.to_uppercase()),
        format!("https://{host}./u/{id}"),
        format!("https://{host}:8443/u/{id}"),
    ] {
        let (activity, uri) = create(&actor, &handle);
        assert_eq!(
            post_signed(&app, &handle, &actor, &activity).await,
            StatusCode::UNAUTHORIZED,
            "{actor}"
        );
        assert!(!stored(&st, &uri).await, "{actor}");
    }
}

#[tokio::test]
async fn the_fetched_document_must_be_the_signer() {
    let st = state().await;
    let handle = forum(&st).await;
    let host = fresh_host();
    let actor = fresh_actor(&host);
    let app = inbox(&st, on(), Some(fresh_actor(&host)));
    let (activity, uri) = create(&actor, &handle);
    assert_eq!(
        post_signed(&app, &handle, &actor, &activity).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!stored(&st, &uri).await);
}

#[tokio::test]
async fn an_activity_id_from_another_authority_is_refused() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(&st, on(), None);
    let host = fresh_host();
    let actor = fresh_actor(&host);
    for id in [
        format!("https://{host}:8443/activities/1"),
        format!("https://me@{host}/activities/1"),
    ] {
        let (mut activity, uri) = create(&actor, &handle);
        activity["id"] = json!(id);
        assert_eq!(
            post_signed(&app, &handle, &actor, &activity).await,
            StatusCode::BAD_REQUEST,
            "{id}"
        );
        assert!(!stored(&st, &uri).await);
    }
}

#[tokio::test]
async fn an_account_over_its_limit_gets_429() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(
        &st,
        InboundConfig {
            per_actor_per_min: 2,
            ..on()
        },
        None,
    );
    let actor = fresh_actor(&fresh_host());
    let mut statuses = Vec::new();
    for _ in 0..3 {
        let (activity, _) = create(&actor, &handle);
        statuses.push(post_signed(&app, &handle, &actor, &activity).await);
    }
    assert_eq!(
        statuses,
        [
            StatusCode::ACCEPTED,
            StatusCode::ACCEPTED,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );
}

#[tokio::test]
async fn new_accounts_on_one_server_share_its_limit() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(
        &st,
        InboundConfig {
            per_host_per_min: 2,
            ..on()
        },
        None,
    );
    let host = fresh_host();
    let mut statuses = Vec::new();
    for _ in 0..3 {
        let actor = fresh_actor(&host);
        let (activity, _) = create(&actor, &handle);
        statuses.push(post_signed(&app, &handle, &actor, &activity).await);
    }
    assert_eq!(statuses[2], StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn with_inbound_off_an_ignored_activity_is_not_marked_seen() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(&st, InboundConfig::default(), None);
    let actor = fresh_actor(&fresh_host());
    let (activity, uri) = create(&actor, &handle);
    assert_eq!(
        post_signed(&app, &handle, &actor, &activity).await,
        StatusCode::ACCEPTED
    );
    assert!(!stored(&st, &uri).await);
    assert!(!seen(&st, &activity).await);

    // Turned on later, the same activity is processed.
    let on_app = inbox(&st, on(), None);
    assert_eq!(
        post_signed(&on_app, &handle, &actor, &activity).await,
        StatusCode::ACCEPTED
    );
    assert!(stored(&st, &uri).await);
}

#[tokio::test]
async fn with_inbound_off_follows_get_no_new_refusals() {
    let st = state().await;
    let handle = forum(&st).await;
    let app = inbox(
        &st,
        InboundConfig {
            per_actor_per_min: 1,
            per_host_per_min: 1,
            ..InboundConfig::default()
        },
        None,
    );
    let actor = fresh_actor(&fresh_host());
    let group = format!("https://{HOST}/actors/{handle}");
    for i in 0..3 {
        // The id is on another host and the rate is over its cap: neither may refuse it.
        let undo = json!({
            "id": format!("https://elsewhere.example/undo/{i}"),
            "type": "Undo", "actor": actor,
            "object": {"type": "Follow", "actor": actor, "object": group},
        });
        assert_eq!(
            post_signed(&app, &handle, &actor, &undo).await,
            StatusCode::ACCEPTED
        );
    }
}
