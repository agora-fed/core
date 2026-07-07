//! # Gateway-level ActivityPub surface — composes auth (DB-backed identity) with the federation
//! crate (pure AP builders + crypto). ADR-0010 W2.1 + W2.2.
//!
//! Routes (mounted at the root, NOT under `/api/v1`, so federation paths look like every other
//! ActivityPub instance):
//!
//! * `GET  /.well-known/webfinger?resource=acct:<handle>@<host>` — RFC 7033 JRD.
//! * `GET  /actors/{handle}`                                     — the Person Actor + publicKey.
//! * `POST /actors/{handle}/inbox`                               — receive signed activities.
//! * `GET  /actors/{handle}/inbox`                               — empty OrderedCollection (stub).
//! * `GET  /actors/{handle}/outbox`                              — empty OrderedCollection (stub).
//! * `GET  /actors/{handle}/followers`                           — OrderedCollection of inbound
//!   ACK'd remote actor URLs.
//! * `GET  /actors/{handle}/following`                           — empty (W2.4).
//!
//! The Follow handshake is **synchronous** in this slice: inbox POST verifies the signature,
//! persists the inbound follow, immediately signs an Accept and posts it to the remote inbox,
//! then marks the follow as ACK'd. A delivery worker with retry is W2.3.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Json as AxumJson, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_app::{AppState, CallerId};
use dsoc_auth::profile::ProfileService;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_federation::signatures::{PublicKey, SignatureHeader, SignatureVerifier};
use dsoc_federation::{
    actor_id, build_signing_string, sign_with_pem, signature_header_value, Actor, ActorRole,
    RsaSha256Verifier,
};
use dsoc_api_contract::ApiResponse;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::federation_feed;
use crate::notifications;

const ACTIVITY_JSON: &str = "application/activity+json";
const JRD_JSON: &str = "application/jrd+json";
const AS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";

/// Per ADR-0010 single-tenant default — the seeded `DemocraciaBR` org.
const DEFAULT_ORG_UUID: uuid::Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Outbound HTTP-client cap: a slow remote inbox must not stall the inbox handler indefinitely.
const REMOTE_FETCH_TIMEOUT_SECS: u64 = 10;
const REMOTE_DELIVERY_TIMEOUT_SECS: u64 = 10;

/// Public ActivityPub surface mounted at the gateway's ROOT (RFC-mandated paths: webfinger,
/// actor docs, inbox/outbox/followers/following). No authentication — these are read by remote
/// instances and crawlers. Inbox POST authenticates via HTTP Signature, not cookies.
pub fn public_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/actors/{handle}", get(actor_handler))
        .route("/actors/{handle}/inbox", post(inbox_post).get(inbox_get_stub))
        .route("/actors/{handle}/outbox", get(outbox_get_populated))
        .route("/actors/{handle}/followers", get(followers_get))
        .route("/actors/{handle}/following", get(following_get))
        .with_state(state)
        // Mastodon does not send Content-Length on streaming bodies; cap at 1 MiB for safety.
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
}

/// Authenticated client surface — paths CITIZENS hit from the front (look up + follow remote
/// actors, post notes). Mounted by the gateway UNDER `/api/v1` so the cookie/identity middleware
/// applies. Caller identity comes from `CallerId` (i.e. the cookie middleware must run first).
pub fn client_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/federation/lookup", get(lookup_remote))
        .route("/me/follow", post(follow_remote))
        .route("/me/notes", post(post_my_note))
        .route("/me/feed", get(get_my_feed))
        .route("/me/like", post(toggle_like))
        .route("/me/boost", post(toggle_boost))
        .route("/me/notifications", get(get_my_notifications))
        .route("/me/notifications/clear", post(clear_my_notifications))
        .route("/notes/context", get(get_thread_context))
        .route("/timelines/tag/{name}", get(get_hashtag_timeline))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct WebFingerQuery {
    resource: String,
}

async fn webfinger_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WebFingerQuery>,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok((user, resource_host)) = dsoc_federation::webfinger::parse_acct(&query.resource) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !resource_host.eq_ignore_ascii_case(&host) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    if svc.find_public_by_handle(org, user).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let actor_url = actor_id(&host, user);
    let jrd = json!({
        "subject": query.resource,
        "links": [
            { "rel": "self", "type": ACTIVITY_JSON, "href": actor_url }
        ]
    });
    match serde_json::to_string(&jrd) {
        Ok(body) => ([(header::CONTENT_TYPE, JRD_JSON)], body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn actor_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    // Content-negotiation: a browser landing on `/actors/<handle>` expects a human page,
    // not JSON-LD. The Fediverse spec (ActivityPub) advertises `application/activity+json` /
    // `application/ld+json`; anything without those (typical browser `text/html,…` header)
    // gets a 302 to the human `/perfil/?u=<handle>` page served by the SPA. The handle rides in
    // a query param (not the path) because the SPA is pure SSG (ADR-0009): it cannot pre-render one
    // HTML file per arbitrary citizen handle at build time, so a single `/perfil/` page reads the
    // handle client-side and hydrates the profile. Substring check covers Mastodon-style
    // `application/activity+json; profile=…` variants. Handles are `[a-z0-9_-]` (CHECK-constrained),
    // so no URL-encoding is needed.
    let wants_activitypub = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            accept.contains("application/activity+json")
                || accept.contains("application/ld+json")
        });
    if !wants_activitypub {
        let location = format!("/perfil/?u={handle}");
        return (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
    }
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(public_pem) = svc
        .ensure_actor_public_key(CitizenId::from_uuid(profile.citizen_id))
        .await
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let actor_url = actor_id(&host, &handle);
    // Remote instances (Mastodon/organica.social) fetch avatar/header from their own servers, so
    // the stored paths (`/media/…`) must be absolutized to `https://{host}/media/…` or they render
    // blank. `absolutize` leaves already-absolute URLs untouched.
    let icon_url = profile.avatar_url.as_deref().map(|u| absolutize(&host, u));
    let image_url = profile.cover_url.as_deref().map(|u| absolutize(&host, u));
    let actor: Actor = Actor::person(
        &host,
        &handle,
        Some(ActorRole::Voter),
        profile.display_name,
    )
    .with_images(icon_url, image_url)
    .with_public_key(PublicKey::main_key(&actor_url, public_pem));
    match serde_json::to_string(&actor) {
        Ok(body) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /actors/{handle}/inbox` — receive a signed activity.
///
/// Flow:
/// 1. Resolve `{handle}` to a public citizen (404 otherwise).
/// 2. Parse the `Signature` header.
/// 3. Fetch the SIGNER's Actor (the `keyId`'s document) to get the signing publicKey.
/// 4. Build the canonical signing string from request headers and verify.
/// 5. Insert-before-act into the inbox idempotency log; duplicates short-circuit with 202.
/// 6. Parse the activity. If `Follow`: persist inbound follow, sign an Accept, POST it to the
///    follower's inbox, mark ACK'd. (Other activity types are politely 202'd as no-ops for now.)
async fn inbox_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
    body: Bytes,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let citizen = CitizenId::from_uuid(profile.citizen_id);

    // --- 1. Parse Signature header ---------------------------------------------------------
    let sig_value = match headers
        .get("signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v,
        None => {
            tracing::warn!("inbox POST without Signature header");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };
    let sig = match SignatureHeader::parse(sig_value) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "malformed Signature header");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // --- 2. Fetch the signer's Actor doc to get their publicKey ----------------------------
    let signer_actor_url = sig.key_id.split('#').next().unwrap_or(&sig.key_id).to_owned();
    let signer_actor = match fetch_remote_actor(&signer_actor_url).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, url = %signer_actor_url, "failed to fetch signer actor");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let signer_public_pem = match signer_actor
        .get("publicKey")
        .and_then(|pk| pk.get("publicKeyPem"))
        .and_then(Value::as_str)
    {
        Some(pem) => pem.to_owned(),
        None => {
            tracing::warn!(url = %signer_actor_url, "signer actor has no publicKey.publicKeyPem");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let signer_key = PublicKey {
        id: sig.key_id.clone(),
        owner: signer_actor_url.clone(),
        public_key_pem: signer_public_pem,
    };

    // --- 3. Build signing string and verify ------------------------------------------------
    let header_pairs: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(n, v)| {
            v.to_str().ok().map(|s| (n.as_str().to_owned(), s.to_owned()))
        })
        .collect();
    let covered: Vec<&str> = sig.headers.iter().map(String::as_str).collect();
    let target = format!("/actors/{handle}/inbox");
    let signing_string = match build_signing_string("post", &target, &header_pairs, &covered) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "covered header missing on inbox request");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    if RsaSha256Verifier
        .verify(&signing_string, &sig.signature, &signer_key)
        .is_err()
    {
        tracing::warn!(key = %sig.key_id, "inbox signature verification failed");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // --- 4. Parse the activity --------------------------------------------------------------
    let activity: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, "inbox body is not JSON");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let activity_id = activity
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if activity_id.is_empty() {
        tracing::warn!("inbox activity missing id");
        return StatusCode::BAD_REQUEST.into_response();
    }

    // --- 5. Idempotency: short-circuit duplicates ------------------------------------------
    match svc.mark_inbox_seen(&activity_id, citizen).await {
        Ok(false) => {
            // Already processed.
            return StatusCode::ACCEPTED.into_response();
        }
        Err(err) => {
            tracing::error!(error = ?err, "failed to mark inbox seen");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Ok(true) => { /* fresh — fall through */ }
    }

    // --- 6. Dispatch by activity type ------------------------------------------------------
    let kind = activity.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "Follow" {
        // The remote inbox is the signer's `inbox` field. Mastodon supports a `sharedInbox`
        // optimization; we accept either but prefer the actor's own inbox for simplicity.
        let remote_inbox = signer_actor
            .get("inbox")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if remote_inbox.is_empty() {
            tracing::warn!(url = %signer_actor_url, "signer actor has no inbox");
            return StatusCode::BAD_GATEWAY.into_response();
        }

        if let Err(err) = svc
            .record_inbound_follow(citizen, &signer_actor_url, &remote_inbox, &activity_id)
            .await
        {
            tracing::error!(error = ?err, "failed to record inbound follow");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        // 0.18.0-beta: user-facing follow notification for the receiver.
        {
            let source_handle = remote_handle_of(&signer_actor, &signer_actor_url);
            let display_name = signer_actor.get("name").and_then(Value::as_str);
            let avatar_url = signer_actor
                .get("icon")
                .and_then(|i| i.get("url"))
                .and_then(Value::as_str);
            let _ = notifications::insert(
                &state.db,
                notifications::NewNotification {
                    citizen_id: citizen.as_uuid(),
                    kind: "follow",
                    source_actor_url: Some(&signer_actor_url),
                    source_handle: &source_handle,
                    source_display_name: display_name,
                    source_avatar_url: avatar_url,
                    object_uri: None,
                    object_preview: None,
                },
            )
            .await;
        }

        // Build, sign, and post the Accept.
        let me_url = actor_id(&host, &handle);
        let accept = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": format!("{me_url}/activities/accept-{}", uuid::Uuid::now_v7()),
            "type": "Accept",
            "actor": me_url,
            "object": activity,
        });
        let private_pem = match svc.read_actor_private_key(citizen).await {
            Ok(pem) => pem,
            Err(err) => {
                tracing::error!(error = ?err, "missing private key for outbound Accept");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        if let Err(err) =
            deliver_signed(&me_url, &private_pem, &remote_inbox, &accept).await
        {
            tracing::warn!(error = ?err, target = %remote_inbox, "Accept delivery failed; will retry on next inbound");
            // Don't fail the inbox call — Mastodon will retry the Follow, and our idempotency
            // table will let us retry the Accept. The follow row stays unaccepted until then.
            return StatusCode::ACCEPTED.into_response();
        }
        if let Err(err) = svc.accept_inbound_follow(citizen, &signer_actor_url).await {
            tracing::error!(error = ?err, "failed to mark follow ACK'd");
        }
    } else if kind == "Accept" {
        // The remote ACK'd a Follow WE sent. The Accept's actor is the remote (signer); the
        // inner object is our original Follow whose `actor` is us. We match on signer URL —
        // there is exactly one pending outbound follow per (citizen, remote actor URL).
        match svc.accept_outbound_follow(citizen, &signer_actor_url).await {
            Ok(true) => tracing::info!(remote = %signer_actor_url, "outbound follow ACK'd"),
            Ok(false) => {
                tracing::debug!(remote = %signer_actor_url, "stray Accept (no matching pending follow)");
            }
            Err(err) => {
                tracing::error!(error = ?err, "failed to mark outbound follow accepted");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else if kind == "Create" {
        // Create(Note) from an actor at least one local citizen follows → upsert into the
        // federated timeline. Anything else (unfollowed stranger, non-Note object) is a
        // logged no-op — we still 202 so the remote doesn't retry forever.
        handle_inbox_create(&state, &signer_actor, &signer_actor_url, &activity).await;
    } else if kind == "Like" || kind == "Announce" {
        // Remote reaction over one of OUR objects → upsert (re-delivery is a no-op).
        handle_inbox_reaction(&state, &signer_actor, &signer_actor_url, kind, &activity_id, &activity).await;
    } else if kind == "Undo" {
        // Undo Follow → the remote unfollowed us. Verify the inner object is a Follow whose
        // actor is the same as the Undo's signer (don't let one user undo another's follow),
        // then delete the row. Idempotent at the DB level (DELETE returns 0 if already gone).
        // Undo Like/Announce → remove the matching remote reaction (same signer scoping).
        let inner_kind = activity
            .get("object")
            .and_then(|o| o.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let inner_actor = activity
            .get("object")
            .and_then(|o| o.get("actor"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if inner_kind == "Follow" && inner_actor == signer_actor_url {
            if let Err(err) = svc.remove_inbound_follow(citizen, &signer_actor_url).await {
                tracing::error!(error = ?err, "failed to remove inbound follow on Undo");
            } else {
                tracing::info!(remote = %signer_actor_url, "inbound follow undone");
            }
        } else if (inner_kind == "Like" || inner_kind == "Announce")
            && (inner_actor.is_empty() || inner_actor == signer_actor_url)
        {
            let db_kind = if inner_kind == "Like" { "like" } else { "boost" };
            let inner_id = activity
                .get("object")
                .and_then(|o| o.get("id"))
                .and_then(Value::as_str);
            let inner_object_uri = activity
                .get("object")
                .and_then(|o| o.get("object"))
                .and_then(object_uri_of);
            if inner_id.is_none() && inner_object_uri.is_none() {
                tracing::debug!("Undo({inner_kind}) without inner id/object — ignored");
            } else {
                match federation_feed::delete_remote_reaction(
                    &state.db,
                    &signer_actor_url,
                    db_kind,
                    inner_id,
                    inner_object_uri.as_deref(),
                )
                .await
                {
                    Ok(n) => tracing::info!(
                        remote = %signer_actor_url,
                        db_kind,
                        removed = n,
                        "remote reaction undone"
                    ),
                    Err(err) => {
                        tracing::error!(error = ?err, "failed to remove remote reaction on Undo");
                    }
                }
            }
        } else {
            tracing::debug!(inner_kind, inner_actor, "ignored Undo for unsupported type or actor mismatch");
        }
    } else {
        tracing::debug!(kind, "ignored unhandled activity type");
    }

    StatusCode::ACCEPTED.into_response()
}

/// `GET /actors/{handle}/inbox` — empty OrderedCollection. Mastodon does GET the inbox URL when
/// the user signs in for the first time as a sanity check; an empty collection is the right
/// answer for "no historical posts here".
async fn inbox_get_stub(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    serve_empty_collection(state, headers, handle, "inbox").await
}

/// `GET /actors/{handle}/outbox` — OrderedCollection of the citizen's public Notes (W2.5).
/// Returns the latest N activities verbatim; the payload column already holds wire-ready JSON.
async fn outbox_get_populated(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (items, total) = match svc
        .list_public_outbox(CitizenId::from_uuid(profile.citizen_id), 40)
        .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let id = format!("{}/outbox", actor_id(&host, &handle));
    let body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": id,
        "type": "OrderedCollection",
        "totalItems": total,
        "orderedItems": items,
    });
    match serde_json::to_string(&body) {
        Ok(s) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], s).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /actors/{handle}/following` — OrderedCollection of remote actor URLs the citizen
/// follows (ACK'd outbound follows). Mastodon reads `totalItems` for the badge.
async fn following_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (items, total) = match svc
        .list_following(CitizenId::from_uuid(profile.citizen_id), 100, 0)
        .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let id = format!("{}/following", actor_id(&host, &handle));
    let body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": id,
        "type": "OrderedCollection",
        "totalItems": total,
        "orderedItems": items,
    });
    match serde_json::to_string(&body) {
        Ok(s) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], s).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Authenticated client API — front-end uses these to look up + follow remote actors (W2.4).
// ---------------------------------------------------------------------------

/// Query for `GET /api/v1/federation/lookup`.
#[derive(Debug, Deserialize)]
struct LookupQuery {
    /// `acct` URI without the scheme (`user@host` or `@user@host`), as a citizen would type it.
    acct: String,
}

/// Sanitized DTO for a resolved remote actor, returned to the front. NEVER carries the raw AP
/// JSON — only the fields the UI actually needs. Avatar URL may be `None` if the remote has none.
#[derive(Debug, Serialize)]
struct RemoteActorDto {
    /// Stable URL of the remote actor (the `id` of the Actor document).
    remote_actor_url: String,
    /// Inbox URL (where we'd POST a Follow). Cached so the follow call doesn't re-fetch.
    inbox_url: String,
    /// `acct:user@host` rendered for the UI to display ("@m@pop.coop").
    handle: String,
    /// Display name (`name` from the Actor doc), if any.
    name: Option<String>,
    /// `preferredUsername` (the local part of the handle), if any.
    preferred_username: Option<String>,
    /// Short summary / bio.
    summary: Option<String>,
    /// Best avatar URL from `icon`/`image`/`avatar` (we look at the first one that resolves).
    avatar_url: Option<String>,
}

/// `GET /api/v1/federation/lookup?acct=@user@host` — webfinger-resolve a remote handle and
/// return a sanitized view. Auth-gated (citizen cookie) so the platform is not a generic
/// fediverse crawler — only logged-in citizens can probe.
async fn lookup_remote(
    State(_state): State<AppState>,
    _caller: CallerId,
    Query(query): Query<LookupQuery>,
) -> Response {
    // Accept "@user@host" with a leading at, or bare "user@host".
    let raw = query.acct.trim_start_matches('@');
    let Some((user, host)) = raw.rsplit_once('@') else {
        return client_error("forneça @usuario@host válido");
    };
    if user.is_empty() || host.is_empty() {
        return client_error("forneça @usuario@host válido");
    }
    // Step 1: webfinger lookup on the remote host.
    let webfinger_url = format!("https://{host}/.well-known/webfinger?resource=acct:{user}@{host}");
    let jrd = match fetch_remote_actor(&webfinger_url).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, host, "webfinger lookup failed");
            return upstream_error("não consegui contatar essa instância");
        }
    };
    // Step 2: pull the `self` link → ActivityStreams Actor URL.
    let self_url = jrd
        .get("links")
        .and_then(Value::as_array)
        .and_then(|links| {
            links.iter().find_map(|l| {
                let rel = l.get("rel").and_then(Value::as_str)?;
                let typ = l.get("type").and_then(Value::as_str)?;
                if rel == "self" && typ.contains("activity") {
                    l.get("href").and_then(Value::as_str)
                } else {
                    None
                }
            })
        });
    let Some(actor_url) = self_url else {
        return upstream_error("instância não expõe um perfil ActivityPub para esse usuário");
    };
    // Step 3: fetch the Actor doc.
    let actor = match fetch_remote_actor(actor_url).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, actor_url, "remote actor fetch failed");
            return upstream_error("não consegui carregar o perfil remoto");
        }
    };
    let dto = sanitize_actor(actor, actor_url, raw);
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// Body for `POST /api/v1/me/follow`.
#[derive(Debug, Deserialize)]
struct FollowRequest {
    /// The remote actor's URL (the `remote_actor_url` returned by `/lookup`).
    remote_actor_url: String,
}

/// `POST /api/v1/me/follow` — send a signed Follow to a remote actor's inbox and persist the
/// outbound row. The Accept comes back asynchronously to our inbox; until then the row stays
/// unACK'd (the UI can show "Solicitação enviada").
async fn follow_remote(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<FollowRequest>,
) -> Response {
    let svc = ProfileService::from_state(&state);
    // The follower must be a public citizen — federation surface is opt-in (ADR-0010).
    let me = match svc
        .find_public_by_handle(caller.org, &handle_of(&svc, caller.citizen).await)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return client_error("torne seu perfil público antes de seguir alguém no fediverso");
        }
    };
    // Make sure we have a key (lazy-generate if first time).
    let _ = match svc.ensure_actor_public_key(caller.citizen).await {
        Ok(p) => p,
        Err(err) => {
            tracing::error!(error = ?err, "ensure_actor_public_key failed");
            return server_error();
        }
    };
    let private_pem = match svc.read_actor_private_key(caller.citizen).await {
        Ok(p) => p,
        Err(err) => {
            tracing::error!(error = ?err, "read_actor_private_key failed");
            return server_error();
        }
    };
    // Fetch the remote actor to learn its inbox URL.
    let remote_actor = match fetch_remote_actor(&body.remote_actor_url).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, "remote fetch failed during follow");
            return upstream_error("instância remota não respondeu");
        }
    };
    let Some(remote_inbox) = remote_actor
        .get("inbox")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return upstream_error("perfil remoto sem inbox");
    };
    // Our handle is what we used in the URL; the host we don't know without the request — pull
    // from PUBLIC_ORIGIN (configured in env; defaults to democracia.social.br).
    let public_origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let me_url = format!(
        "{}/actors/{}",
        public_origin.trim_end_matches('/'),
        me.handle.as_deref().unwrap_or(&me.public_handle)
    );
    let activity_id = format!("{me_url}/activities/follow-{}", uuid::Uuid::now_v7());
    let follow = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": activity_id,
        "type": "Follow",
        "actor": me_url,
        "object": body.remote_actor_url,
    });
    if let Err(err) = deliver_signed(&me_url, &private_pem, &remote_inbox, &follow).await {
        tracing::warn!(error = ?err, target = %remote_inbox, "outbound Follow delivery failed");
        return upstream_error("não consegui entregar o pedido de seguir");
    }
    if let Err(err) = svc
        .record_outbound_follow(caller.citizen, &body.remote_actor_url, &remote_inbox, &activity_id)
        .await
    {
        tracing::error!(error = ?err, "persist outbound follow failed");
        return server_error();
    }
    (
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(json!({ "status": "pending" }))),
    )
        .into_response()
}

/// Body for `POST /api/v1/me/notes`.
#[derive(Debug, Deserialize)]
struct PostNoteRequest {
    /// The note's text content. Server-side validation: non-empty, max 5000 chars.
    content: String,
    /// 0.18.0: parent Note object URI (for threaded replies). Optional.
    #[serde(default)]
    in_reply_to_uri: Option<String>,
    /// 0.18.0: Mastodon-style sensitive flag (opt-in).
    #[serde(default)]
    sensitive: bool,
    /// 0.18.0: content-warning header (max 500 chars, trimmed server-side).
    #[serde(default)]
    spoiler_text: Option<String>,
}

/// `POST /api/v1/me/notes` — publish a public Note. Wraps the content in a `Create(Note)`,
/// persists into the outbox, fans out one delivery row per ACK'd inbound follower; the worker
/// drains the queue asynchronously. Returns `{activity_id, fanout_count, status: "queued"}`.
async fn post_my_note(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<PostNoteRequest>,
) -> Response {
    let svc = ProfileService::from_state(&state);
    // The author must be a public citizen (federation surface is opt-in per ADR-0010).
    let me = match svc
        .find_public_by_handle(caller.org, &handle_of(&svc, caller.citizen).await)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return client_error(
                "torne seu perfil público antes de publicar (em Configurações)",
            );
        }
    };
    // Make sure the keypair exists so the worker can sign — first publish triggers lazy gen.
    if let Err(err) = svc.ensure_actor_public_key(caller.citizen).await {
        tracing::error!(error = ?err, "ensure_actor_public_key failed");
        return server_error();
    }
    let public_origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let me_url = format!(
        "{}/actors/{}",
        public_origin.trim_end_matches('/'),
        me.handle.as_deref().unwrap_or(&me.public_handle)
    );
    match svc
        .create_public_note(
            caller.citizen,
            &me_url,
            &public_origin,
            &body.content,
            body.in_reply_to_uri.as_deref(),
            body.sensitive,
            body.spoiler_text.as_deref(),
        )
        .await
    {
        Ok((activity_id, fanout)) => {
            // 0.18.0-beta: fire in-app notifications for local recipients.
            // (a) reply-to-local: if the parent's owner is a local citizen, ping them.
            // (b) mention-to-local: for each extracted mention whose actor URL points at
            //     our origin, ping the matching citizen. Skip self-notifications.
            let object_id = activity_id.replace("/activities/note-", "/objects/");
            let preview = notifications::preview_from_html(&body.content);
            let sender_handle = me.handle.clone().unwrap_or_else(|| me.public_handle.clone());
            let sender_display = me.display_name.clone();
            let sender_avatar = me.avatar_url.clone();
            if let Some(reply_uri) = body.in_reply_to_uri.as_deref().filter(|s| !s.is_empty()) {
                if let Ok(Some(owner_id)) =
                    notifications::find_owner_of_object(&state.db, reply_uri).await
                {
                    if owner_id != caller.citizen.as_uuid() {
                        let _ = notifications::insert(
                            &state.db,
                            notifications::NewNotification {
                                citizen_id: owner_id,
                                kind: "reply",
                                source_actor_url: Some(&me_url),
                                source_handle: &sender_handle,
                                source_display_name: sender_display.as_deref(),
                                source_avatar_url: sender_avatar.as_deref(),
                                object_uri: Some(&object_id),
                                object_preview: Some(&preview),
                            },
                        )
                        .await;
                    }
                }
            }
            for m in dsoc_federation::extract_mentions(&body.content) {
                let target_url = m.best_actor_url(&public_origin);
                if let Ok(Some(mentioned_id)) =
                    notifications::find_local_citizen_by_actor_url(
                        &state.db,
                        &target_url,
                        &public_origin,
                    )
                    .await
                {
                    if mentioned_id != caller.citizen.as_uuid() {
                        let _ = notifications::insert(
                            &state.db,
                            notifications::NewNotification {
                                citizen_id: mentioned_id,
                                kind: "mention",
                                source_actor_url: Some(&me_url),
                                source_handle: &sender_handle,
                                source_display_name: sender_display.as_deref(),
                                source_avatar_url: sender_avatar.as_deref(),
                                object_uri: Some(&object_id),
                                object_preview: Some(&preview),
                            },
                        )
                        .await;
                    }
                }
            }
            (
                StatusCode::ACCEPTED,
                Json(ApiResponse::ok(json!({
                    "activity_id": activity_id,
                    "fanout_count": fanout,
                    "status": "queued",
                }))),
            )
                .into_response()
        }
        Err(dsoc_core::Error::Validation(msg)) => client_error(&msg),
        Err(err) => {
            tracing::error!(error = ?err, "create_public_note failed");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Federated feed + reactions (ADR-0010 W2.6) — client surface + inbox plumbing.
// ---------------------------------------------------------------------------

/// Query for `GET /api/v1/me/feed`.
#[derive(Debug, Deserialize)]
struct FeedQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// Query for `GET /api/v1/me/notifications?limit&offset`.
#[derive(Debug, Deserialize)]
struct NotifQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `GET /api/v1/me/notifications` — the citizen's in-app notification feed
/// (mention/reply/favourite/reblog/follow). Newest first, paginated.
async fn get_my_notifications(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<NotifQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(30).clamp(1, 50);
    let offset = query.offset.unwrap_or(0).max(0);
    match notifications::list_for_citizen(
        &state.db,
        caller.citizen.as_uuid(),
        limit,
        offset,
    )
    .await
    {
        Ok(items) => {
            let unread = notifications::unread_count(&state.db, caller.citizen.as_uuid())
                .await
                .unwrap_or(0);
            (
                StatusCode::OK,
                Json(ApiResponse::ok(json!({
                    "items": items,
                    "unread_count": unread,
                }))),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "notifications query failed");
            server_error()
        }
    }
}

/// `POST /api/v1/me/notifications/clear` — mark every unread notification as read.
async fn clear_my_notifications(
    State(state): State<AppState>,
    caller: CallerId,
) -> Response {
    match notifications::mark_all_read(&state.db, caller.citizen.as_uuid()).await {
        Ok(n) => (StatusCode::OK, Json(ApiResponse::ok(json!({ "cleared": n })))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "notifications clear failed");
            server_error()
        }
    }
}

/// Query for hashtag timeline (`GET /api/v1/timelines/tag/{name}?limit&offset`).
#[derive(Debug, Deserialize)]
struct HashtagQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `GET /api/v1/timelines/tag/{name}` — public timeline of Notes indexed under
/// `#name` (normalized). Open to unauthenticated callers so the tag page can
/// be shared as a link.
async fn get_hashtag_timeline(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<HashtagQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(30).clamp(1, 50);
    let offset = query.offset.unwrap_or(0).max(0);
    // Normalize the incoming tag the same way the extractor does — so `#Saúde`
    // and `saude` collide (and match the stored `tag_normalized`).
    let normalized = dsoc_federation::extract_hashtags(&format!("#{name}"))
        .into_iter()
        .next()
        .map(|h| h.normalized)
        .unwrap_or_else(|| name.to_lowercase());
    if normalized.is_empty() {
        return client_error("hashtag inválida");
    }
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match federation_feed::list_hashtag_timeline(
        &state.db,
        &normalized,
        &media_base,
        limit,
        offset,
    )
    .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({
                "tag": normalized,
                "items": items,
            }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "hashtag timeline query failed");
            server_error()
        }
    }
}

/// Query for `GET /api/v1/notes/context?uri=`.
#[derive(Debug, Deserialize)]
struct ThreadContextQuery {
    uri: String,
}

/// `GET /api/v1/notes/context?uri=<object_uri>` — descendants of a Note (the root plus every
/// reply subtree), depth-capped. Used by the single-status thread view. 0.18.0 does not walk
/// ancestors — call this on the topmost URI you know and the front expands from there.
async fn get_thread_context(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<ThreadContextQuery>,
) -> Response {
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match federation_feed::list_thread_context(
        &state.db,
        &query.uri,
        caller.citizen.as_uuid(),
        &media_base,
    )
    .await
    {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "thread context query failed");
            server_error()
        }
    }
}

/// `GET /api/v1/me/feed?limit&offset` — the citizen's merged federated timeline: their own local
/// Notes, Notes of local citizens they follow, and remote Notes of fediverse actors they follow,
/// newest first. `limit` caps at 50 (default 20).
async fn get_my_feed(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<FeedQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let offset = query.offset.unwrap_or(0).max(0);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match federation_feed::list_feed(
        &state.db,
        caller.citizen.as_uuid(),
        &public_origin(),
        &media_base,
        limit,
        offset,
    )
    .await
    {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "feed query failed");
            server_error()
        }
    }
}

/// Body for `POST /api/v1/me/like` and `POST /api/v1/me/boost`.
#[derive(Debug, Deserialize)]
struct ReactionRequest {
    /// The Note object's URI (the `object_uri` from a feed item).
    object_uri: String,
}

/// Which reaction a toggle endpoint drives. Maps 1:1 to the DB `kind` and the AP activity type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReactionKind {
    Like,
    Boost,
}

impl ReactionKind {
    const fn db_kind(self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Boost => "boost",
        }
    }

    const fn ap_type(self) -> &'static str {
        match self {
            Self::Like => "Like",
            Self::Boost => "Announce",
        }
    }
}

/// What the background delivery task should send to the remote author.
#[derive(Debug)]
enum ReactionDelivery {
    /// Deliver a fresh `Like`/`Announce` with this activity id.
    Set { activity_id: String },
    /// Deliver an `Undo` wrapping the original activity id.
    Undo { prev_activity_id: String },
}

/// `POST /api/v1/me/like` — toggle the caller's Like on an object. Response:
/// `{ "liked": bool, "like_count": n }`.
async fn toggle_like(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<ReactionRequest>,
) -> Response {
    toggle_reaction(state, caller, body, ReactionKind::Like).await
}

/// `POST /api/v1/me/boost` — toggle the caller's Boost (`Announce`) on an object. Response:
/// `{ "boosted": bool, "boost_count": n }`.
async fn toggle_boost(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<ReactionRequest>,
) -> Response {
    toggle_reaction(state, caller, body, ReactionKind::Boost).await
}

/// Shared toggle body: flip the local `federation_reaction` row, then best-effort deliver the
/// signed `Like`/`Announce` (or `Undo`) to the remote author's inbox IN THE BACKGROUND — a slow
/// or dead remote never fails the citizen's click.
async fn toggle_reaction(
    state: AppState,
    caller: CallerId,
    body: ReactionRequest,
    kind: ReactionKind,
) -> Response {
    let object_uri = body.object_uri.trim().to_owned();
    if object_uri.is_empty() || object_uri.len() > 2048 {
        return client_error("informe um object_uri válido");
    }
    let db_kind = kind.db_kind();
    let citizen = caller.citizen.as_uuid();
    let existing =
        match federation_feed::find_local_reaction(&state.db, citizen, &object_uri, db_kind).await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(error = ?err, "reaction lookup failed");
                return server_error();
            }
        };
    let active = if let Some(prev_activity_id) = existing {
        // Toggle OFF.
        if let Err(err) =
            federation_feed::delete_local_reaction(&state.db, citizen, &object_uri, db_kind).await
        {
            tracing::error!(error = ?err, "reaction delete failed");
            return server_error();
        }
        spawn_reaction_delivery(
            state.clone(),
            caller,
            object_uri.clone(),
            kind,
            ReactionDelivery::Undo { prev_activity_id },
        );
        false
    } else {
        // Toggle ON. The activity id embeds OUR actor URL so the remote can dereference it.
        let svc = ProfileService::from_state(&state);
        let handle = handle_of(&svc, caller.citizen).await;
        let me_url = format!("{}/actors/{handle}", public_origin());
        let activity_id = format!("{me_url}/activities/{db_kind}-{}", uuid::Uuid::now_v7());
        let now = state.clock.now();
        if let Err(err) = federation_feed::insert_local_reaction(
            &state.db,
            citizen,
            &object_uri,
            db_kind,
            &activity_id,
            now,
        )
        .await
        {
            tracing::error!(error = ?err, "reaction insert failed");
            return server_error();
        }
        spawn_reaction_delivery(
            state.clone(),
            caller,
            object_uri.clone(),
            kind,
            ReactionDelivery::Set { activity_id },
        );
        true
    };
    let count = match federation_feed::count_reactions(&state.db, &object_uri, db_kind).await {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = ?err, "reaction count failed");
            return server_error();
        }
    };
    let payload = match kind {
        ReactionKind::Like => json!({ "liked": active, "like_count": count }),
        ReactionKind::Boost => json!({ "boosted": active, "boost_count": count }),
    };
    (StatusCode::OK, Json(ApiResponse::ok(payload))).into_response()
}

/// Fire-and-forget the federation delivery of a reaction toggle. Failures are logged only —
/// the citizen's local state is already committed and the endpoint contract says best-effort.
fn spawn_reaction_delivery(
    state: AppState,
    caller: CallerId,
    object_uri: String,
    kind: ReactionKind,
    action: ReactionDelivery,
) {
    tokio::spawn(async move {
        if let Err(err) = deliver_reaction(&state, caller, &object_uri, kind, &action).await {
            tracing::warn!(
                error = %err,
                object_uri = %object_uri,
                "entrega best-effort de reação federada falhou"
            );
        }
    });
}

/// Deliver the signed `Like`/`Announce`/`Undo` to the remote author's inbox. A LOCAL object
/// (not present in `federation_timeline_entry`) short-circuits to `Ok(())` — nothing to federate.
async fn deliver_reaction(
    state: &AppState,
    caller: CallerId,
    object_uri: &str,
    kind: ReactionKind,
    action: &ReactionDelivery,
) -> Result<(), FederationError> {
    let author_actor_url = federation_feed::find_timeline_actor(&state.db, object_uri)
        .await
        .map_err(|e| FederationError::Http(format!("db: {e}")))?;
    let Some(author_actor_url) = author_actor_url else {
        // Local (or unknown) object — the reaction stays local.
        return Ok(());
    };
    let svc = ProfileService::from_state(state);
    let handle = handle_of(&svc, caller.citizen).await;
    let me_url = format!("{}/actors/{handle}", public_origin());
    svc.ensure_actor_public_key(caller.citizen)
        .await
        .map_err(|e| FederationError::Http(format!("key: {e}")))?;
    let private_pem = svc
        .read_actor_private_key(caller.citizen)
        .await
        .map_err(|e| FederationError::Http(format!("key: {e}")))?;
    let remote_actor = fetch_remote_actor(&author_actor_url).await?;
    let Some(inbox) = remote_actor.get("inbox").and_then(Value::as_str) else {
        return Err(FederationError::Http("remote author has no inbox".to_owned()));
    };
    let activity = match action {
        ReactionDelivery::Set { activity_id } => json!({
            "@context": AS_CONTEXT,
            "id": activity_id,
            "type": kind.ap_type(),
            "actor": me_url,
            "object": object_uri,
        }),
        ReactionDelivery::Undo { prev_activity_id } => json!({
            "@context": AS_CONTEXT,
            "id": format!("{me_url}/activities/undo-{}", uuid::Uuid::now_v7()),
            "type": "Undo",
            "actor": me_url,
            "object": {
                "id": prev_activity_id,
                "type": kind.ap_type(),
                "actor": me_url,
                "object": object_uri,
            },
        }),
    };
    deliver_signed(&me_url, &private_pem, inbox, &activity).await
}

/// Inbox side of `Create(Note)`: store the remote Note in the shared timeline when the author is
/// followed by at least one local citizen. Always returns (the caller 202s regardless) — every
/// skip/failure is logged with its reason.
async fn handle_inbox_create(
    state: &AppState,
    signer_actor: &Value,
    signer_actor_url: &str,
    activity: &Value,
) {
    let Some(object) = activity.get("object") else {
        tracing::debug!("Create without object — ignored");
        return;
    };
    let object_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    if object_type != "Note" {
        tracing::debug!(object_type, "Create of non-Note object — ignored");
        return;
    }
    let Some(object_uri) = object.get("id").and_then(Value::as_str) else {
        tracing::debug!("Create(Note) without object id — ignored");
        return;
    };
    // Anti-spoof: the Note must be attributed to the SIGNER (the actor whose key verified).
    let attributed = object
        .get("attributedTo")
        .and_then(Value::as_str)
        .or_else(|| activity.get("actor").and_then(Value::as_str))
        .unwrap_or("");
    if attributed != signer_actor_url {
        tracing::warn!(attributed, signer = %signer_actor_url, "Create(Note) attribution mismatch — dropped");
        return;
    }
    match federation_feed::anyone_follows(&state.db, signer_actor_url).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(actor = %signer_actor_url, "Create(Note) from unfollowed actor — ignored");
            return;
        }
        Err(err) => {
            tracing::error!(error = ?err, "follow check failed on inbound Create");
            return;
        }
    }
    // Content: cap at 64 KiB as delivered, then allowlist-sanitize; final char-cap keeps the
    // DB CHECK (65536 chars) honest even when sanitization expands anchors.
    let raw = object.get("content").and_then(Value::as_str).unwrap_or("");
    let capped = federation_feed::truncate_bytes(raw, federation_feed::CONTENT_MAX_BYTES);
    let mut content = federation_feed::sanitize_html(capped);
    if content.chars().count() > 65_536 {
        content = content.chars().take(65_536).collect();
    }
    let published = object
        .get("published")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(chrono::Utc::now, |d| d.with_timezone(&chrono::Utc));
    let display_name = signer_actor.get("name").and_then(Value::as_str);
    let avatar_url = signer_actor
        .get("icon")
        .and_then(|i| i.get("url"))
        .and_then(Value::as_str);
    let handle = remote_handle_of(signer_actor, signer_actor_url);
    // 0.18.0: extract Mastodon-parity fields from the Note object. All optional.
    let in_reply_to = object.get("inReplyTo").and_then(Value::as_str);
    let sensitive = object
        .get("sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // `summary` is Mastodon's CW header. Truncate to 1024 chars (permissive cap; local publish
    // uses 500). Empty string = treated as absent.
    let spoiler_owned: Option<String> = object
        .get("summary")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(1024).collect());
    if let Err(err) = federation_feed::upsert_timeline_entry(
        &state.db,
        object_uri,
        signer_actor_url,
        &handle,
        display_name,
        avatar_url,
        &content,
        published,
        chrono::Utc::now(),
        in_reply_to,
        sensitive,
        spoiler_owned.as_deref(),
    )
    .await
    {
        tracing::error!(error = ?err, object_uri, "failed to store remote Note");
        return;
    }
    tracing::info!(actor = %signer_actor_url, object_uri, "remote Note stored in timeline");
    // 0.18.0-beta: user-facing notifications for local recipients of this remote Note.
    // (a) reply to one of OUR objects → notify the object's owner.
    // (b) mention pointing at a local actor URL → notify that citizen.
    let po = public_origin();
    let preview = notifications::preview_from_html(&content);
    let src_handle = remote_handle_of(signer_actor, signer_actor_url);
    if let Some(reply_uri) = in_reply_to {
        if let Ok(Some(owner_id)) =
            notifications::find_owner_of_object(&state.db, reply_uri).await
        {
            let _ = notifications::insert(
                &state.db,
                notifications::NewNotification {
                    citizen_id: owner_id,
                    kind: "reply",
                    source_actor_url: Some(signer_actor_url),
                    source_handle: &src_handle,
                    source_display_name: display_name,
                    source_avatar_url: avatar_url,
                    object_uri: Some(object_uri),
                    object_preview: Some(&preview),
                },
            )
            .await;
        }
    }
    // Index #hashtags and @mentions carried in the AP `tag[]` array. Falls back to text
    // extraction from `content` if the remote omitted the array (older Pleroma versions).
    let now = chrono::Utc::now();
    let mut saw_tag_array = false;
    if let Some(tags) = object.get("tag").and_then(Value::as_array) {
        saw_tag_array = !tags.is_empty();
        for tag in tags {
            let ttype = tag.get("type").and_then(Value::as_str).unwrap_or("");
            match ttype {
                "Mention" => {
                    let href = tag.get("href").and_then(Value::as_str).unwrap_or("");
                    let name = tag.get("name").and_then(Value::as_str).unwrap_or("");
                    if !href.is_empty() && !name.is_empty() {
                        let _ = federation_feed::upsert_mention(
                            &state.db,
                            object_uri,
                            href,
                            name.trim_start_matches('@'),
                            now,
                        )
                        .await;
                        // If the mention resolves to a local citizen, notify them.
                        if let Ok(Some(mentioned_id)) =
                            notifications::find_local_citizen_by_actor_url(&state.db, href, &po)
                                .await
                        {
                            let _ = notifications::insert(
                                &state.db,
                                notifications::NewNotification {
                                    citizen_id: mentioned_id,
                                    kind: "mention",
                                    source_actor_url: Some(signer_actor_url),
                                    source_handle: &src_handle,
                                    source_display_name: display_name,
                                    source_avatar_url: avatar_url,
                                    object_uri: Some(object_uri),
                                    object_preview: Some(&preview),
                                },
                            )
                            .await;
                        }
                    }
                }
                "Hashtag" => {
                    let name = tag
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim_start_matches('#')
                        .trim();
                    if !name.is_empty() {
                        // Reuse local extractor normalization so #Saúde and #saude collide.
                        let normalized = dsoc_federation::extract_hashtags(&format!("#{name}"))
                            .into_iter()
                            .next()
                            .map(|h| h.normalized)
                            .unwrap_or_else(|| name.to_lowercase());
                        let _ = federation_feed::upsert_hashtag(
                            &state.db,
                            object_uri,
                            &normalized,
                            name,
                            now,
                        )
                        .await;
                    }
                }
                _ => {}
            }
        }
    }
    if !saw_tag_array {
        // Fallback for remotes that don't populate tag[] — extract from content.
        for m in dsoc_federation::extract_mentions(&content) {
            let _ = federation_feed::upsert_mention(
                &state.db,
                object_uri,
                &m.best_actor_url(""),
                &m.handle,
                now,
            )
            .await;
        }
        for h in dsoc_federation::extract_hashtags(&content) {
            let _ = federation_feed::upsert_hashtag(
                &state.db,
                object_uri,
                &h.normalized,
                &h.original,
                now,
            )
            .await;
        }
    }
}

/// Inbox side of `Like` / `Announce`: record a remote reaction over one of OUR objects.
async fn handle_inbox_reaction(
    state: &AppState,
    signer_actor: &Value,
    signer_actor_url: &str,
    kind: &str,
    activity_id: &str,
    activity: &Value,
) {
    let Some(object_uri) = activity.get("object").and_then(object_uri_of) else {
        tracing::debug!(kind, "reaction without object — ignored");
        return;
    };
    let db_kind = if kind == "Like" { "like" } else { "boost" };
    match federation_feed::is_our_object(&state.db, &object_uri).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(kind, object_uri = %object_uri, "reaction over non-local object — ignored");
            return;
        }
        Err(err) => {
            tracing::error!(error = ?err, "object ownership check failed on inbound reaction");
            return;
        }
    }
    if let Err(err) = federation_feed::upsert_remote_reaction(
        &state.db,
        signer_actor_url,
        &object_uri,
        db_kind,
        activity_id,
        chrono::Utc::now(),
    )
    .await
    {
        tracing::error!(error = ?err, "failed to store remote reaction");
        return;
    }
    tracing::info!(remote = %signer_actor_url, db_kind, object_uri = %object_uri, "remote reaction stored");
    // 0.18.0-beta: user-facing notification for the object's owner.
    let notif_kind = if kind == "Like" { "favourite" } else { "reblog" };
    if let Ok(Some(owner_id)) = notifications::find_owner_of_object(&state.db, &object_uri).await {
        let handle = remote_handle_of(signer_actor, signer_actor_url);
        let display_name = signer_actor.get("name").and_then(Value::as_str);
        let avatar_url = signer_actor
            .get("icon")
            .and_then(|i| i.get("url"))
            .and_then(Value::as_str);
        let _ = notifications::insert(
            &state.db,
            notifications::NewNotification {
                citizen_id: owner_id,
                kind: notif_kind,
                source_actor_url: Some(signer_actor_url),
                source_handle: &handle,
                source_display_name: display_name,
                source_avatar_url: avatar_url,
                object_uri: Some(&object_uri),
                object_preview: None,
            },
        )
        .await;
    }
}

/// Pull an object URI out of an AP `object` field, which the wire gives either as a bare string
/// or as an embedded object carrying an `id`.
fn object_uri_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => v.get("id").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// Render a remote actor's fediverse handle as `user@host` (the feed contract's remote shape).
fn remote_handle_of(actor: &Value, actor_url: &str) -> String {
    let user = actor
        .get("preferredUsername")
        .and_then(Value::as_str)
        .map_or_else(
            || {
                actor_url
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("desconhecido")
                    .to_owned()
            },
            str::to_owned,
        );
    let host = reqwest::Url::parse(actor_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "remoto".to_owned());
    format!("{user}@{host}")
}

/// The public origin this instance federates under (env `PUBLIC_ORIGIN`), trailing-slash-free.
fn public_origin() -> String {
    std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
        .trim_end_matches('/')
        .to_owned()
}

/// Resolve the caller's handle from their profile row (needed because `follow_remote` builds a
/// URL with it). Caller is taken from middleware, not from any user input.
async fn handle_of(svc: &ProfileService, citizen: CitizenId) -> String {
    // `find_public_by_handle` reads by handle; we need to go the other way. Use the existing
    // public profile read which always works for the authenticated caller.
    if let Ok(p) = svc.get(citizen, OrgId::from_uuid(DEFAULT_ORG_UUID)).await {
        return p.handle.unwrap_or(p.public_handle);
    }
    // Fallback to opaque handle — never happens in practice because the caller is authenticated.
    format!("u-{}", citizen.as_uuid().simple())
}

/// Reduce a fetched remote Actor JSON to the fields the UI needs. Strips raw HTML, picks the
/// first usable avatar URL, and falls back gracefully when fields are absent.
fn sanitize_actor(actor: Value, actor_url: &str, acct: &str) -> RemoteActorDto {
    let name = actor
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let preferred_username = actor
        .get("preferredUsername")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let summary = actor
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let avatar_url = actor
        .get("icon")
        .and_then(|i| i.get("url"))
        .or_else(|| actor.get("image").and_then(|i| i.get("url")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let inbox_url = actor
        .get("inbox")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    RemoteActorDto {
        remote_actor_url: actor_url.to_owned(),
        inbox_url,
        handle: format!("@{acct}"),
        name,
        preferred_username,
        summary,
        avatar_url,
    }
}

fn client_error(message: &str) -> Response {
    let body = ApiResponse::<()>::fail("invalid_input", message);
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn upstream_error(message: &str) -> Response {
    let body = ApiResponse::<()>::fail("upstream_error", message);
    (StatusCode::BAD_GATEWAY, Json(body)).into_response()
}

fn server_error() -> Response {
    let body = ApiResponse::<()>::fail("storage_error", "Erro interno do servidor.");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

async fn serve_empty_collection(
    state: AppState,
    headers: HeaderMap,
    handle: String,
    suffix: &str,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    if svc.find_public_by_handle(org, &handle).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let id = format!("{}/{suffix}", actor_id(&host, &handle));
    let body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": id,
        "type": "OrderedCollection",
        "totalItems": 0,
        "orderedItems": [],
    });
    match serde_json::to_string(&body) {
        Ok(s) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], s).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /actors/{handle}/followers` — OrderedCollection of remote actor URLs that follow us.
/// Mastodon reads `totalItems` for the follower count badge.
async fn followers_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (items, total) = match svc
        .list_followers(CitizenId::from_uuid(profile.citizen_id), 100, 0)
        .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let id = format!("{}/followers", actor_id(&host, &handle));
    let body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": id,
        "type": "OrderedCollection",
        "totalItems": total,
        "orderedItems": items,
    });
    match serde_json::to_string(&body) {
        Ok(s) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], s).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// --- Helpers ----------------------------------------------------------------------------------

/// Fetch a remote ActivityPub Actor document (Mastodon, Pleroma, etc.). Returns the parsed
/// JSON; the caller picks the fields it needs (publicKey, inbox, etc.).
async fn fetch_remote_actor(url: &str) -> Result<Value, FederationError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REMOTE_FETCH_TIMEOUT_SECS))
        .user_agent("DemocraciaBR/0.4 (+https://democracia.social.br)")
        .build()
        .map_err(|e| FederationError::Http(e.to_string()))?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, ACTIVITY_JSON)
        .send()
        .await
        .map_err(|e| FederationError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(FederationError::Http(format!(
            "remote returned {}",
            resp.status()
        )));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| FederationError::Http(format!("json: {e}")))
}

/// Sign and POST an activity to a remote inbox. The covered headers are
/// `(request-target) host date digest`, the same set Mastodon emits and expects.
pub(crate) async fn deliver_signed(
    sender_actor_url: &str,
    sender_private_pem: &str,
    target_inbox_url: &str,
    activity: &Value,
) -> Result<(), FederationError> {
    let body = serde_json::to_vec(activity)
        .map_err(|e| FederationError::Http(format!("serialize: {e}")))?;
    let digest_b64 = {
        use sha2::Digest;
        base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(&body))
    };
    let digest_value = format!("SHA-256={digest_b64}");
    let date = chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();

    let parsed = reqwest::Url::parse(target_inbox_url)
        .map_err(|e| FederationError::Http(format!("bad inbox url: {e}")))?;
    let host = match (parsed.host_str(), parsed.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => return Err(FederationError::Http("inbox url has no host".to_owned())),
    };
    let path = if parsed.query().is_some() {
        format!("{}?{}", parsed.path(), parsed.query().unwrap_or(""))
    } else {
        parsed.path().to_owned()
    };

    let signing_headers = vec![
        ("Host".to_owned(), host.clone()),
        ("Date".to_owned(), date.clone()),
        ("Digest".to_owned(), digest_value.clone()),
    ];
    let covered = ["(request-target)", "host", "date", "digest"];
    let signing_string = build_signing_string("post", &path, &signing_headers, &covered)
        .map_err(|e| FederationError::Http(format!("signing string: {e}")))?;
    let signature_b64 = sign_with_pem(sender_private_pem, &signing_string)
        .map_err(|e| FederationError::Http(format!("sign: {e}")))?;
    let key_id = format!("{sender_actor_url}#main-key");
    let signature_header = signature_header_value(&key_id, &covered, &signature_b64);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REMOTE_DELIVERY_TIMEOUT_SECS))
        .user_agent("DemocraciaBR/0.4 (+https://democracia.social.br)")
        .build()
        .map_err(|e| FederationError::Http(e.to_string()))?;
    let resp = client
        .post(target_inbox_url)
        .header(reqwest::header::CONTENT_TYPE, ACTIVITY_JSON)
        .header(reqwest::header::HOST, host)
        .header("Date", date)
        .header("Digest", digest_value)
        .header("Signature", signature_header)
        .body(body)
        .send()
        .await
        .map_err(|e| FederationError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(FederationError::Http(format!(
            "remote inbox returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        )));
    }
    Ok(())
}

use base64::Engine;

/// Errors from outbound federation operations. Public-safe; the inbox handler logs the detail
/// and returns a generic status to the wire.
#[derive(Debug)]
pub(crate) enum FederationError {
    Http(String),
}

impl std::fmt::Display for FederationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(s) => write!(f, "federation http error: {s}"),
        }
    }
}

impl std::error::Error for FederationError {}

fn host_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// Turn a possibly-relative media path (`/media/…`) into an absolute `https://{host}/…` URL.
/// Already-absolute URLs (`http://`, `https://`) pass through untouched. Needed because the
/// federation surface hands these URLs to remote instances, which cannot resolve site-relative paths.
fn absolutize(host: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else if let Some(rest) = url.strip_prefix('/') {
        format!("https://{host}/{rest}")
    } else {
        format!("https://{host}/{url}")
    }
}
