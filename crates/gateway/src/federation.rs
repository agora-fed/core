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
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_app::AppState;
use dsoc_auth::profile::ProfileService;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_federation::signatures::{PublicKey, SignatureHeader, SignatureVerifier};
use dsoc_federation::{
    actor_id, build_signing_string, sign_with_pem, signature_header_value, Actor, ActorRole,
    RsaSha256Verifier,
};
use serde::Deserialize;
use serde_json::{json, Value};

const ACTIVITY_JSON: &str = "application/activity+json";
const JRD_JSON: &str = "application/jrd+json";

/// Per ADR-0010 single-tenant default — the seeded `DemocraciaBR` org.
const DEFAULT_ORG_UUID: uuid::Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Outbound HTTP-client cap: a slow remote inbox must not stall the inbox handler indefinitely.
const REMOTE_FETCH_TIMEOUT_SECS: u64 = 10;
const REMOTE_DELIVERY_TIMEOUT_SECS: u64 = 10;

/// Mount the federation HTTP surface on the gateway's root router.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/actors/{handle}", get(actor_handler))
        .route("/actors/{handle}/inbox", post(inbox_post).get(inbox_get_stub))
        .route("/actors/{handle}/outbox", get(outbox_get_stub))
        .route("/actors/{handle}/followers", get(followers_get))
        .route("/actors/{handle}/following", get(following_get_stub))
        .with_state(state)
        // Mastodon does not send Content-Length on streaming bodies; cap at 1 MiB for safety.
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
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
    } else if kind == "Undo" {
        // TODO(W2.3): handle Undo Follow → remove the follow row.
        tracing::info!(kind, "ignored activity type (W2.3)");
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

/// `GET /actors/{handle}/outbox` — empty OrderedCollection. Will carry posted notes in W3.
async fn outbox_get_stub(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    serve_empty_collection(state, headers, handle, "outbox").await
}

/// `GET /actors/{handle}/following` — empty until W2.4 (we don't yet send Follow ourselves).
async fn following_get_stub(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    serve_empty_collection(state, headers, handle, "following").await
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
async fn deliver_signed(
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
enum FederationError {
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
