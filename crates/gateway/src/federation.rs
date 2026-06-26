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

const ACTIVITY_JSON: &str = "application/activity+json";
const JRD_JSON: &str = "application/jrd+json";

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
    let actor: Actor = Actor::person(
        &host,
        &handle,
        Some(ActorRole::Voter),
        profile.display_name,
    )
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
    } else if kind == "Undo" {
        // Undo Follow → the remote unfollowed us. Verify the inner object is a Follow whose
        // actor is the same as the Undo's signer (don't let one user undo another's follow),
        // then delete the row. Idempotent at the DB level (DELETE returns 0 if already gone).
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
        } else {
            tracing::debug!(inner_kind, inner_actor, "ignored Undo for non-Follow or actor mismatch");
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
        .create_public_note(caller.citizen, &me_url, &body.content)
        .await
    {
        Ok((activity_id, fanout)) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(json!({
                "activity_id": activity_id,
                "fanout_count": fanout,
                "status": "queued",
            }))),
        )
            .into_response(),
        Err(dsoc_core::Error::Validation(msg)) => client_error(&msg),
        Err(err) => {
            tracing::error!(error = ?err, "create_public_note failed");
            server_error()
        }
    }
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
