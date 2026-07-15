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
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use dsoc_auth::profile::ProfileService;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_federation::signatures::{PublicKey, SignatureHeader, SignatureVerifier};
use dsoc_federation::{
    actor_id, build_signing_string, sign_with_pem, signature_header_value, Actor, ActorRole,
    RsaSha256Verifier,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::discovery;
use crate::federation_feed;
use crate::note_media;
use crate::notifications;
use crate::polls;

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
        .route("/actors/{handle}/objects/{id}", get(object_handler))
        .route(
            "/actors/{handle}/inbox",
            post(inbox_post).get(inbox_get_stub),
        )
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
        .route("/federation/actor-outbox", get(get_remote_outbox))
        .route("/me/follow", post(follow_remote))
        .route("/me/follow/status", get(follow_status))
        .route("/me/social/following", get(my_following_list))
        .route("/me/social/followers", get(my_followers_list))
        .route("/me/bulk_follow", post(bulk_follow))
        .route(
            "/me/notes",
            post(post_my_note)
                .delete(delete_my_note)
                .patch(patch_my_note),
        )
        .route("/me/feed", get(get_my_feed))
        .route("/me/like", post(toggle_like))
        .route("/me/boost", post(toggle_boost))
        .route("/me/notifications", get(get_my_notifications))
        .route("/me/notifications/clear", post(clear_my_notifications))
        .route("/me/media", post(post_my_media))
        .route("/me/actor/refresh", post(refresh_my_actor))
        .route("/me/notes/vote", post(post_poll_vote))
        .route("/notes/context", get(get_thread_context))
        .route("/timelines/tag/{name}", get(get_hashtag_timeline))
        .route("/search", get(search_all))
        .route("/search/hashtags", get(search_hashtags))
        .route("/search/mentions", get(search_mentions))
        .route("/trends/hashtags", get(trending_hashtags))
        .route("/directory", get(directory_endpoint))
        .route("/suggestions/follow", get(follow_suggestions_endpoint))
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
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

/// `GET /actors/{handle}/objects/{id}` — dereferenceable AP Note (federation) OR a friendly
/// HTML page with Open Graph tags (for social preview cards when the link is shared on
/// Mastodon, WhatsApp, Slack, etc). Prior to this handler, this URL returned 404 — link
/// previews were empty and remote servers couldn't refetch our notes.
async fn object_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((handle, id)): Path<(String, String)>,
) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Reconstrói o activity_id que está armazenado (o object_id que estamos servindo é
    // derivado dele: /activities/note-<uuid> ↔ /objects/<uuid>).
    let activity_id = format!("https://{host}/actors/{handle}/activities/note-{id}");
    let object_url = format!("https://{host}/actors/{handle}/objects/{id}");
    let row: Result<Option<(Value,)>, _> = sqlx::query_as::<_, (Value,)>(
        r"SELECT payload FROM federation_outbox_entry WHERE activity_id = $1",
    )
    .bind(&activity_id)
    .fetch_optional(&state.db)
    .await;
    let payload = match row {
        Ok(Some((p,))) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "object_handler DB");
            return server_error();
        }
    };
    let Some(note) = payload.get("object").cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Content-negotiation: AP client → Note JSON-LD.
    let wants_ap = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| {
            a.contains("application/activity+json") || a.contains("application/ld+json")
        });
    if wants_ap {
        return match serde_json::to_string(&note) {
            Ok(body) => ([(header::CONTENT_TYPE, ACTIVITY_JSON)], body).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    // Browser: HTML com OG tags + redirect pra /publicacao/?uri=<object_url>.
    let content_html = note.get("content").and_then(Value::as_str).unwrap_or("");
    let plain = strip_html(content_html);
    let title = truncate_chars(&plain, 80);
    let desc = truncate_chars(&plain, 200);
    let published = note.get("published").and_then(Value::as_str).unwrap_or("");
    // Avatar do autor pra og:image (opcional; a card ainda aparece sem).
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let avatar = svc
        .find_public_by_handle(org, &handle)
        .await
        .ok()
        .and_then(|p| p.avatar_url)
        .map(|u| absolutize(&host, &u));
    let publicacao_url = format!("/publicacao/?uri={}", urlencode(&object_url));
    let og_title = escape_html(&format!("@{handle} · {title}"));
    let og_desc = escape_html(&desc);
    let canon = escape_html(&object_url);
    let redirect_target = escape_html(&publicacao_url);
    let og_image_tag = avatar
        .as_deref()
        .map(|u| format!(r#"<meta property="og:image" content="{}">"#, escape_html(u)))
        .unwrap_or_default();
    let article_time = if published.is_empty() {
        String::new()
    } else {
        format!(
            r#"<meta property="article:published_time" content="{}">"#,
            escape_html(published)
        )
    };
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · @{handle} · DemocraciaBR</title>
<meta name="description" content="{og_desc}">
<meta property="og:type" content="article">
<meta property="og:site_name" content="DemocraciaBR">
<meta property="og:title" content="{og_title}">
<meta property="og:description" content="{og_desc}">
<meta property="og:url" content="{canon}">
{og_image_tag}
{article_time}
<meta name="twitter:card" content="summary">
<meta name="twitter:title" content="{og_title}">
<meta name="twitter:description" content="{og_desc}">
<link rel="canonical" href="{canon}">
<link rel="alternate" type="application/activity+json" href="{canon}">
<meta http-equiv="refresh" content="0; url={redirect_target}">
<style>body{{font:14px system-ui;color:#334;margin:2rem}} a{{color:#115c2d}}</style>
</head>
<body>
<p>Redirecionando para <a href="{redirect_target}">a publicação</a>…</p>
</body>
</html>"#,
        title = escape_html(&title),
        handle = escape_html(&handle),
        og_title = og_title,
        og_desc = og_desc,
        canon = canon,
        og_image_tag = og_image_tag,
        article_time = article_time,
        redirect_target = redirect_target,
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
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
            accept.contains("application/activity+json") || accept.contains("application/ld+json")
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
    // Bio → Mastodon `summary` (HTML). The plain-text bio in the DB uses
    // blank-line paragraphs and hard line breaks; `plain_bio_to_html` turns
    // that into safe `<p>` blocks with `<br>` for intra-paragraph breaks.
    let summary_html = profile
        .bio
        .as_deref()
        .map(dsoc_federation::plain_bio_to_html)
        .filter(|s| !s.is_empty());
    // Human-readable profile URL (Mastodon's "view profile" link) — points at
    // the SPA route, distinct from the JSON-LD id.
    let profile_url = format!("https://{host}/perfil/?u={handle}");
    let actor: Actor = Actor::person(&host, &handle, Some(ActorRole::Voter), profile.display_name)
        .with_summary(summary_html)
        .with_url(Some(profile_url))
        .with_published(Some(profile.created_at.to_rfc3339()))
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
    let sig_value = match headers.get("signature").and_then(|v| v.to_str().ok()) {
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
    let signer_actor_url = sig
        .key_id
        .split('#')
        .next()
        .unwrap_or(&sig.key_id)
        .to_owned();
    // Server-wide block (migration 0508): se o host do signer está em
    // server_domain_block com severity='suspend', rejeitamos a atividade
    // ANTES de qualquer fetch. Silence-only não bloqueia entrega — só
    // esconde no feed público.
    if let Some(host) = host_from_url(&signer_actor_url) {
        let blocked: bool = sqlx::query_scalar(
            r"SELECT EXISTS (
                 SELECT 1 FROM server_domain_block
                  WHERE severity = 'suspend' AND domain = $1)",
        )
        .bind(host.to_ascii_lowercase())
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
        if blocked {
            tracing::info!(host, "inbox POST rejected: server_domain_block suspend");
            return StatusCode::FORBIDDEN.into_response();
        }
    }
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
            v.to_str()
                .ok()
                .map(|s| (n.as_str().to_owned(), s.to_owned()))
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
        if let Err(err) = deliver_signed(&me_url, &private_pem, &remote_inbox, &accept).await {
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
        handle_inbox_reaction(
            &state,
            &signer_actor,
            &signer_actor_url,
            kind,
            &activity_id,
            &activity,
        )
        .await;
    } else if kind == "Delete" {
        // Remote author (or their instance) deleted a Note. Soft-delete the row on our side
        // so the feed drops it and the thread view shows a tombstone. Signer-scoped: only
        // the object's author can delete it — we match on both actor_url and object URI.
        if let Some(target_uri) = activity.get("object").and_then(object_uri_of) {
            let now = chrono::Utc::now();
            let _ = sqlx::query(
                r"UPDATE federation_timeline_entry
                     SET deleted_at = $2
                   WHERE object_uri = $1 AND actor_url = $3",
            )
            .bind(&target_uri)
            .bind(now)
            .bind(&signer_actor_url)
            .execute(&state.db)
            .await;
            tracing::info!(remote = %signer_actor_url, target_uri, "remote Note tombstoned");
        }
    } else if kind == "Update" {
        // Remote author edited a Note. Rewrite the cached content_html + stamp edited_at.
        // Only Notes today; anything else is a logged no-op.
        if let Some(inner) = activity.get("object") {
            let inner_type = inner.get("type").and_then(Value::as_str).unwrap_or("");
            let inner_short = inner_type.rsplit(':').next().unwrap_or(inner_type);
            if inner_short == "Note" {
                if let Some(uri) = inner.get("id").and_then(Value::as_str) {
                    let raw = inner.get("content").and_then(Value::as_str).unwrap_or("");
                    let capped =
                        federation_feed::truncate_bytes(raw, federation_feed::CONTENT_MAX_BYTES);
                    let content = federation_feed::sanitize_html(capped);
                    let sensitive = inner
                        .get("sensitive")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let spoiler = inner
                        .get("summary")
                        .and_then(Value::as_str)
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.chars().take(1024).collect::<String>());
                    let now = chrono::Utc::now();
                    let _ = sqlx::query(
                        r"UPDATE federation_timeline_entry
                             SET content_html = $2,
                                 sensitive    = $3,
                                 spoiler_text = $4,
                                 edited_at    = $5
                           WHERE object_uri = $1 AND actor_url = $6",
                    )
                    .bind(uri)
                    .bind(&content)
                    .bind(sensitive)
                    .bind(spoiler.as_deref())
                    .bind(now)
                    .bind(&signer_actor_url)
                    .execute(&state.db)
                    .await;
                    tracing::info!(remote = %signer_actor_url, uri, "remote Note edit applied");
                }
            }
        }
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
            let db_kind = if inner_kind == "Like" {
                "like"
            } else {
                "boost"
            };
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
            tracing::debug!(
                inner_kind,
                inner_actor,
                "ignored Undo for unsupported type or actor mismatch"
            );
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
    /// What the citizen typed: `user@host`, `@user@host`, or a pasted `https://` profile URL.
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

/// `GET /api/v1/federation/lookup?acct=…` — resolve a remote profile and return a sanitized
/// view. Auth-gated (citizen cookie) so the platform is not a generic fediverse crawler —
/// only logged-in citizens can probe.
///
/// Mastodon-parity input forms:
/// - `@user@host` ou `user@host` → WebFinger + Actor fetch;
/// - `https://host/@user` (ou qualquer URL de perfil/actor) → Actor fetch direto com
///   content negotiation, sem WebFinger — igual ao "colar a URL na busca" do Mastodon.
async fn lookup_remote(
    State(_state): State<AppState>,
    _caller: CallerId,
    Query(query): Query<LookupQuery>,
) -> Response {
    let input = query.acct.trim();
    if input.starts_with("https://") {
        return lookup_remote_by_url(input).await;
    }
    if input.starts_with("http://") {
        return client_error("use https:// — instâncias do fediverso não federam por http");
    }
    // Accept "@user@host" with a leading at, or bare "user@host".
    let raw = input.trim_start_matches('@');
    let Some((user, host)) = raw.rsplit_once('@') else {
        return client_error("forneça @usuario@host ou a URL https:// do perfil");
    };
    if user.is_empty() || host.is_empty() {
        return client_error("forneça @usuario@host ou a URL https:// do perfil");
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

/// Resolve a pasted profile URL (`https://host/@user`, `https://host/users/user` or the
/// actor URL itself) by fetching the document directly with ActivityPub content negotiation.
/// The handle shown to the UI comes from `preferredUsername@host-do-actor-id`, which is what
/// Mastodon also displays before a canonical WebFinger round-trip.
async fn lookup_remote_by_url(url: &str) -> Response {
    let actor = match fetch_remote_actor(url).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = ?err, url, "remote actor fetch by url failed");
            return upstream_error("não consegui carregar esse endereço");
        }
    };
    // Um perfil ActivityPub tem inbox; URL de post/coleção não vira perfil.
    if actor.get("inbox").and_then(Value::as_str).is_none() {
        return upstream_error("esse endereço não é um perfil ActivityPub");
    }
    let actor_url = actor
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(url)
        .to_owned();
    let user = actor
        .get("preferredUsername")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let host = reqwest::Url::parse(&actor_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let acct = if user.is_empty() || host.is_empty() {
        // Sem preferredUsername não dá pra montar user@host — mostra a URL mesmo.
        actor_url.trim_start_matches("https://").to_owned()
    } else {
        format!("{user}@{host}")
    };
    let dto = sanitize_actor(actor, &actor_url, &acct);
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// Query for `GET /api/v1/federation/actor-outbox`.
#[derive(Debug, Deserialize)]
struct OutboxProxyQuery {
    /// The remote actor's stable URL (as returned by `/federation/lookup.remote_actor_url`).
    actor_url: String,
}

/// A note surfaced by the outbox proxy. Sanitization of `content_html` happens client-side
/// (same path as feed notes) so this DTO is just a transport shell.
#[derive(Debug, Clone, Serialize)]
struct RemoteNoteDto {
    /// The AP object id — used as a stable key on the client.
    id: String,
    /// The human-facing permalink on the origin instance (if the remote exposes one).
    url: Option<String>,
    /// Raw HTML from the remote. UI runs `sanitizeNoteHtml` before render.
    content_html: String,
    /// ISO-8601 timestamp from the remote (`published`).
    published_at: Option<String>,
    /// If it's a reply, the URI of the parent post.
    in_reply_to: Option<String>,
}

/// 60 s is short enough that a visitor rarely sees stale notes, long enough that we don't
/// hammer the remote instance on refresh loops. In-memory only; loss on pod restart is fine.
const OUTBOX_CACHE_TTL_SECS: u64 = 60;

/// Cache map: actor_url → (fetched_at, notes). Grows unbounded in practice but the working
/// set is tiny (visited remote actors). A proper LRU is a future concern.
static OUTBOX_CACHE: std::sync::LazyLock<
    tokio::sync::RwLock<
        std::collections::HashMap<String, (std::time::Instant, Vec<RemoteNoteDto>)>,
    >,
> = std::sync::LazyLock::new(|| tokio::sync::RwLock::new(std::collections::HashMap::new()));

async fn fetch_actor_outbox(actor_url: &str) -> Result<Vec<RemoteNoteDto>, String> {
    // Step 1: get the actor doc → find the `outbox` URL.
    let actor = fetch_remote_actor(actor_url)
        .await
        .map_err(|e| format!("actor fetch: {e:?}"))?;
    let outbox_url = actor
        .get("outbox")
        .and_then(Value::as_str)
        .ok_or_else(|| "actor sem campo outbox".to_string())?;
    // Step 2: fetch the OrderedCollection wrapper (has `first` pointing at page 1).
    let collection = fetch_remote_actor(outbox_url)
        .await
        .map_err(|e| format!("outbox collection: {e:?}"))?;
    let first_page_url = collection
        .get("first")
        .and_then(|f| {
            f.as_str()
                .or_else(|| f.get("id").and_then(Value::as_str))
                .map(str::to_owned)
        })
        .ok_or_else(|| "outbox sem primeira página".to_string())?;
    // Step 3: fetch first page → orderedItems.
    let page = fetch_remote_actor(&first_page_url)
        .await
        .map_err(|e| format!("outbox page: {e:?}"))?;
    let items = page
        .get("orderedItems")
        .and_then(Value::as_array)
        .ok_or_else(|| "página sem orderedItems".to_string())?;
    let mut notes = Vec::new();
    for item in items.iter().take(20) {
        // Mastodon wraps Notes in Create; some instances emit Note directly. Announce (boost)
        // and Delete are skipped in this slice.
        let item_type = item.get("type").and_then(Value::as_str);
        let object = match item_type {
            Some("Create") => item.get("object"),
            Some("Note") => Some(item),
            _ => continue,
        };
        let Some(obj) = object else { continue };
        if obj.get("type").and_then(Value::as_str) != Some("Note") {
            continue;
        }
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let content_html = obj
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() || content_html.is_empty() {
            continue;
        }
        let published_at = obj
            .get("published")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let url = obj.get("url").and_then(Value::as_str).map(str::to_owned);
        let in_reply_to = obj
            .get("inReplyTo")
            .and_then(Value::as_str)
            .map(str::to_owned);
        notes.push(RemoteNoteDto {
            id,
            url,
            content_html,
            published_at,
            in_reply_to,
        });
    }
    Ok(notes)
}

async fn get_actor_outbox_cached(actor_url: &str) -> Result<Vec<RemoteNoteDto>, String> {
    let now = std::time::Instant::now();
    {
        let cache = OUTBOX_CACHE.read().await;
        if let Some((when, notes)) = cache.get(actor_url) {
            if now.duration_since(*when) < std::time::Duration::from_secs(OUTBOX_CACHE_TTL_SECS) {
                return Ok(notes.clone());
            }
        }
    }
    let notes = fetch_actor_outbox(actor_url).await?;
    {
        let mut cache = OUTBOX_CACHE.write().await;
        cache.insert(actor_url.to_string(), (now, notes.clone()));
    }
    Ok(notes)
}

/// `GET /api/v1/federation/actor-outbox?actor_url=…` — pull the last ~20 notes from a remote
/// actor's outbox (Mastodon/Pleroma/…) so the front can render the timeline INSIDE
/// DemocraciaBR instead of redirecting. Auth-gated (same rationale as `/lookup`).
async fn get_remote_outbox(
    State(_state): State<AppState>,
    _caller: CallerId,
    Query(query): Query<OutboxProxyQuery>,
) -> Response {
    let url = query.actor_url.trim();
    if !url.starts_with("https://") {
        return client_error("URL do actor precisa ser https://…");
    }
    match get_actor_outbox_cached(url).await {
        Ok(notes) => (StatusCode::OK, Json(ApiResponse::ok(notes))).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, actor_url = url, "outbox proxy failed");
            upstream_error("não consegui carregar as notas desse perfil")
        }
    }
}

/// Body for `POST /api/v1/me/follow`.
#[derive(Debug, Deserialize)]
struct FollowRequest {
    /// The remote actor's URL (the `remote_actor_url` returned by `/lookup`).
    remote_actor_url: String,
}

/// Query for `GET /api/v1/me/follow/status?actor_url=…`.
#[derive(Debug, Deserialize)]
struct FollowStatusQuery {
    actor_url: String,
}

#[derive(Debug, Serialize)]
struct FollowStatusDto {
    following: bool,
    /// Pending: enviamos Follow mas o Accept remoto ainda não chegou.
    pending: bool,
}

/// `GET /api/v1/me/follow/status?actor_url=…` — a UI usa isso ao pintar um
/// perfil remoto pra saber se o botão deve dizer "Seguir" ou "Seguindo".
async fn follow_status(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<FollowStatusQuery>,
) -> Response {
    let actor_url = query.actor_url.trim();
    if actor_url.is_empty() {
        return client_error("informe actor_url");
    }
    let row: Result<Option<(bool,)>, _> = sqlx::query_as::<_, (bool,)>(
        r"SELECT (accepted_at IS NOT NULL) AS accepted
            FROM federation_follow
           WHERE citizen_id = $1
             AND direction  = 'outbound'
             AND remote_actor_url = $2
           LIMIT 1",
    )
    .bind(caller.citizen.as_uuid())
    .bind(actor_url)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some((accepted,))) => (
            StatusCode::OK,
            Json(ApiResponse::ok(FollowStatusDto {
                following: accepted,
                pending: !accepted,
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(FollowStatusDto {
                following: false,
                pending: false,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "follow_status query failed");
            server_error()
        }
    }
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
        .record_outbound_follow(
            caller.citizen,
            &body.remote_actor_url,
            &remote_inbox,
            &activity_id,
        )
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

/// Query for `DELETE /api/v1/me/notes?uri=<object_uri>` and PATCH.
#[derive(Debug, Deserialize)]
struct NoteRefQuery {
    uri: String,
}

/// Body for `PATCH /api/v1/me/notes?uri=…`. Only mutable text-level fields for
/// now — media edits require a separate flow (upload + reattach) and are
/// deferred to a later cut.
#[derive(Debug, Deserialize)]
struct PatchNoteRequest {
    /// New text content. Same 1–3000 char validation as post_my_note.
    content: String,
    #[serde(default)]
    sensitive: bool,
    #[serde(default)]
    spoiler_text: Option<String>,
}

/// Body for `POST /api/v1/me/notes`.
#[derive(Debug, Deserialize)]
struct PostNoteRequest {
    /// The note's text content. Server-side validation: non-empty, max 3000 chars.
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
    /// 0.18.0-gamma: media_attachment ids (max 4) to bind to this Note.
    #[serde(default)]
    media_ids: Vec<uuid::Uuid>,
    /// 0.18.0-gamma: per-id alt_text updates applied server-side before binding.
    /// Length must match `media_ids` order; empty entries leave the existing
    /// row untouched.
    #[serde(default)]
    media_alts: Vec<String>,
    /// 0.18.0-rc1: optional poll — flips the AP object to Question.
    #[serde(default)]
    poll: Option<polls::PollInput>,
}

/// `POST /api/v1/me/notes` — publish a public Note. Wraps the content in a `Create(Note)`,
/// persists into the outbox, fans out one delivery row per ACK'd inbound follower; the worker
/// drains the queue asynchronously. Returns `{activity_id, fanout_count, status: "queued"}`.
/// Anti-spam: cidadão pode publicar no máximo 1 nota a cada 15 min. Rate
/// limit reforçado no back — mesma regra é anunciada no cadastro pra
/// setar expectativa desde a inscrição.
const POST_RATE_LIMIT_SECS: i64 = 15 * 60;

async fn post_my_note(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<PostNoteRequest>,
) -> Response {
    // Rate limit ANTES de tudo — barato, dispensa carregar o profile.
    match sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        r"SELECT created_at FROM federation_outbox_entry
           WHERE citizen_id = $1
           ORDER BY created_at DESC
           LIMIT 1",
    )
    .bind(caller.citizen.as_uuid())
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(last_at)) => {
            let elapsed = chrono::Utc::now() - last_at;
            let remaining = POST_RATE_LIMIT_SECS - elapsed.num_seconds();
            if remaining > 0 {
                let mins = ((remaining as f64) / 60.0).ceil() as i64;
                let msg = if mins <= 1 {
                    "aguarde 1 minuto pra publicar de novo (limite de 1 publicação a cada 15 min)"
                        .to_owned()
                } else {
                    format!(
                        "aguarde {} min pra publicar de novo (limite de 1 publicação a cada 15 min)",
                        mins
                    )
                };
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(ApiResponse::<()>::fail("http_429", msg.as_str())),
                )
                    .into_response();
            }
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(error = ?err, "post_my_note rate limit check failed");
            // Falha DB é seguridade — bloqueia por segurança.
            return server_error();
        }
    };
    let svc = ProfileService::from_state(&state);
    // The author must be a public citizen (federation surface is opt-in per ADR-0010).
    let me = match svc
        .find_public_by_handle(caller.org, &handle_of(&svc, caller.citizen).await)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return client_error("torne seu perfil público antes de publicar (em Configurações)");
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
            let object_id = activity_id.replace("/activities/note-", "/objects/");
            // 0.18.0-gamma: update alt_text (best-effort, per-id, only when the
            // caller sent a non-empty value) then bind media to the note.
            for (i, mid) in body.media_ids.iter().enumerate() {
                if let Some(alt) = body
                    .media_alts
                    .get(i)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    let _ = note_media::update_alt_text(&state.db, *mid, alt).await;
                }
            }
            if !body.media_ids.is_empty() {
                if let Err(err) =
                    note_media::attach_to_note(&state.db, &object_id, &body.media_ids).await
                {
                    tracing::warn!(error = ?err, "failed to attach media to note");
                }
                // 0.18.0-rc1: rewrite the outbox payload so the delivery worker
                // ships `attachment[]` on the wire — federation instances render
                // the images we just uploaded.
                let media_base =
                    std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
                if let Err(err) = note_media::update_outbox_payload_with_attachments(
                    &state.db,
                    &activity_id,
                    &object_id,
                    &media_base,
                )
                .await
                {
                    tracing::warn!(error = ?err, "failed to patch outbox with attachment[]");
                }
            }
            // 0.18.0-rc1: poll — persist + flip AP object to Question.
            if let Some(poll_input) = &body.poll {
                match polls::create_from_input(&state.db, &object_id, poll_input).await {
                    Ok(_) => {
                        if let Err(err) = polls::update_outbox_payload_with_question(
                            &state.db,
                            &activity_id,
                            &object_id,
                        )
                        .await
                        {
                            tracing::warn!(error = ?err, "failed to patch outbox with Question");
                        }
                    }
                    Err(err) => {
                        // Note is already stored — log the poll failure but return
                        // the note so the client at least sees "note saved without poll".
                        tracing::warn!(error = ?err, "poll creation failed for note");
                    }
                }
            }
            // 0.18.0-beta: fire in-app notifications for local recipients.
            // (a) reply-to-local: if the parent's owner is a local citizen, ping them.
            // (b) mention-to-local: for each extracted mention whose actor URL points at
            //     our origin, ping the matching citizen. Skip self-notifications.
            let preview = notifications::preview_from_html(&body.content);
            let sender_handle = me
                .handle
                .clone()
                .unwrap_or_else(|| me.public_handle.clone());
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
                if let Ok(Some(mentioned_id)) = notifications::find_local_citizen_by_actor_url(
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

/// Body for `POST /api/v1/me/notes/vote?uri=…`.
#[derive(Debug, Deserialize)]
struct PollVoteRequest {
    option_ids: Vec<uuid::Uuid>,
}

/// `POST /api/v1/me/notes/vote?uri=<object_uri>` — cast a ballot on a Note's
/// poll. Body carries the chosen option ids (1 for single-choice, 1..=N for
/// multi-select). Returns the refreshed poll DTO.
async fn post_poll_vote(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<NoteRefQuery>,
    AxumJson(body): AxumJson<PollVoteRequest>,
) -> Response {
    let uri = query.uri.trim();
    if uri.is_empty() {
        return client_error("uri obrigatória");
    }
    let svc = ProfileService::from_state(&state);
    let handle_now = handle_of(&svc, caller.citizen).await;
    let po = public_origin();
    let voter_url = format!("{}/actors/{}", po.trim_end_matches('/'), handle_now);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match polls::cast_vote(&state.db, uri, &voter_url, &body.option_ids, &media_base).await {
        Ok(dto) => (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response(),
        Err(err) => match err {
            polls::PollError::Db(_) => {
                tracing::error!(error = ?err, "vote persistence failed");
                server_error()
            }
            other => client_error(&other.user_message()),
        },
    }
}

/// `DELETE /api/v1/me/notes?uri=<object_uri>` — soft-delete a Note the caller
/// owns (sets `deleted_at`) and fan out a signed `Delete(Note)` activity to
/// every ACK'd inbound follower so remote timelines drop it too.
async fn delete_my_note(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<NoteRefQuery>,
) -> Response {
    let uri = query.uri.trim();
    if uri.is_empty() {
        return client_error("uri obrigatória");
    }
    // Ownership + not-already-deleted check + fetch the raw activity id.
    let row = match sqlx::query_as::<_, (String,)>(
        r"SELECT activity_id FROM federation_outbox_entry
           WHERE citizen_id = $1
             AND (activity_id = $2 OR payload->'object'->>'id' = $2)
             AND deleted_at IS NULL
           LIMIT 1",
    )
    .bind(caller.citizen.as_uuid())
    .bind(uri)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return client_error("publicação não encontrada ou já apagada"),
        Err(err) => {
            tracing::error!(error = ?err, "delete-note lookup failed");
            return server_error();
        }
    };
    let activity_id = row.0;
    let now = chrono::Utc::now();
    if let Err(err) =
        sqlx::query(r"UPDATE federation_outbox_entry SET deleted_at = $2 WHERE activity_id = $1")
            .bind(&activity_id)
            .bind(now)
            .execute(&state.db)
            .await
    {
        tracing::error!(error = ?err, "delete-note soft-delete failed");
        return server_error();
    }
    // Best-effort federate. If the delivery loop is unreachable the local
    // delete still stands and the front pretends success — that's Mastodon's
    // behaviour too.
    let svc = ProfileService::from_state(&state);
    let handle_now = handle_of(&svc, caller.citizen).await;
    let po = public_origin();
    let actor_url = format!("{}/actors/{}", po.trim_end_matches('/'), handle_now);
    let delete_activity = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor_url}/activities/delete-{}", uuid::Uuid::now_v7()),
        "type": "Delete",
        "actor": actor_url,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "object": {
            "id": uri,
            "type": "Tombstone",
        },
    });
    let inboxes: Vec<String> = sqlx::query_scalar::<_, String>(
        r"SELECT remote_inbox_url FROM federation_follow
           WHERE citizen_id = $1
             AND direction = 'inbound'
             AND accepted_at IS NOT NULL
             AND remote_inbox_url IS NOT NULL",
    )
    .bind(caller.citizen.as_uuid())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if !inboxes.is_empty() {
        if let Ok(pem) = svc.read_actor_private_key(caller.citizen).await {
            for inbox in &inboxes {
                let _ = deliver_signed(&actor_url, &pem, inbox, &delete_activity).await;
            }
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "deleted": true,
            "delivered_to": inboxes.len(),
        }))),
    )
        .into_response()
}

/// `PATCH /api/v1/me/notes?uri=<object_uri>` — edit the text/CW of a Note the
/// caller owns. Stamps `edited_at`, rewrites the outbox payload with the new
/// `content` / `sensitive` / `summary`, then emits an `Update(Note)`. Mastodon
/// clients read the freshly patched Note object and re-render.
async fn patch_my_note(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<NoteRefQuery>,
    AxumJson(body): AxumJson<PatchNoteRequest>,
) -> Response {
    let uri = query.uri.trim();
    if uri.is_empty() {
        return client_error("uri obrigatória");
    }
    let content = body.content.trim().to_owned();
    if content.is_empty() {
        return client_error("digite alguma coisa antes de salvar");
    }
    if content.chars().count() > 3_000 {
        return client_error("o texto está muito longo (máx 3000 caracteres)");
    }
    let spoiler = body
        .spoiler_text
        .as_deref()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(500).collect::<String>());
    // Fetch owned + not-deleted outbox row.
    let row = match sqlx::query_as::<_, (String, serde_json::Value)>(
        r"SELECT activity_id, payload FROM federation_outbox_entry
           WHERE citizen_id = $1
             AND (activity_id = $2 OR payload->'object'->>'id' = $2)
             AND deleted_at IS NULL
           LIMIT 1",
    )
    .bind(caller.citizen.as_uuid())
    .bind(uri)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return client_error("publicação não encontrada"),
        Err(err) => {
            tracing::error!(error = ?err, "patch-note lookup failed");
            return server_error();
        }
    };
    let (activity_id, mut payload) = row;
    let now = chrono::Utc::now();
    // Rewrite the inner Note object in-place.
    if let Some(object) = payload.get_mut("object").and_then(|o| o.as_object_mut()) {
        object.insert("content".into(), serde_json::Value::String(content.clone()));
        object.insert(
            "updated".into(),
            serde_json::Value::String(now.to_rfc3339()),
        );
        if body.sensitive {
            object.insert("sensitive".into(), serde_json::Value::Bool(true));
        } else {
            object.remove("sensitive");
        }
        if let Some(cw) = spoiler.as_deref() {
            object.insert("summary".into(), serde_json::Value::String(cw.to_owned()));
        } else {
            object.remove("summary");
        }
    }
    if let Err(err) = sqlx::query(
        r"UPDATE federation_outbox_entry
             SET payload = $2, edited_at = $3, sensitive = $4, spoiler_text = $5
           WHERE activity_id = $1",
    )
    .bind(&activity_id)
    .bind(&payload)
    .bind(now)
    .bind(body.sensitive)
    .bind(spoiler.as_deref())
    .execute(&state.db)
    .await
    {
        tracing::error!(error = ?err, "patch-note update failed");
        return server_error();
    }
    // Fan out Update(Note).
    let svc = ProfileService::from_state(&state);
    let handle_now = handle_of(&svc, caller.citizen).await;
    let po = public_origin();
    let actor_url = format!("{}/actors/{}", po.trim_end_matches('/'), handle_now);
    let inner_object = payload
        .get("object")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let update = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor_url}/activities/update-{}", uuid::Uuid::now_v7()),
        "type": "Update",
        "actor": actor_url,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{actor_url}/followers")],
        "object": inner_object,
    });
    let inboxes: Vec<String> = sqlx::query_scalar::<_, String>(
        r"SELECT remote_inbox_url FROM federation_follow
           WHERE citizen_id = $1
             AND direction = 'inbound'
             AND accepted_at IS NOT NULL
             AND remote_inbox_url IS NOT NULL",
    )
    .bind(caller.citizen.as_uuid())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if !inboxes.is_empty() {
        if let Ok(pem) = svc.read_actor_private_key(caller.citizen).await {
            for inbox in &inboxes {
                let _ = deliver_signed(&actor_url, &pem, inbox, &update).await;
            }
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "updated": true,
            "delivered_to": inboxes.len(),
        }))),
    )
        .into_response()
}

/// `POST /api/v1/me/actor/refresh` — emit an `Update(Person)` activity to every
/// inbound follower so remote instances (Mastodon et al.) re-fetch the Actor
/// document and pick up avatar/cover/summary/name changes. Otherwise their
/// cache stays stale until it expires (~24h in Mastodon). Returns
/// `{ delivered_to: N }`.
async fn refresh_my_actor(State(state): State<AppState>, caller: CallerId) -> Response {
    let svc = ProfileService::from_state(&state);
    let handle_now = handle_of(&svc, caller.citizen).await;
    let profile = match svc.find_public_by_handle(caller.org, &handle_now).await {
        Ok(p) => p,
        Err(_) => {
            return client_error("torne seu perfil público antes de propagar");
        }
    };
    let host = std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let actor_url = actor_id(&host, &handle_now);
    // Build the fresh Actor doc identical to what /actors/{handle} returns.
    let public_pem = match svc.ensure_actor_public_key(caller.citizen).await {
        Ok(pem) => pem,
        Err(_) => return server_error(),
    };
    let icon_url = profile.avatar_url.as_deref().map(|u| absolutize(&host, u));
    let image_url = profile.cover_url.as_deref().map(|u| absolutize(&host, u));
    let summary_html = profile
        .bio
        .as_deref()
        .map(dsoc_federation::plain_bio_to_html)
        .filter(|s| !s.is_empty());
    let profile_url = format!("https://{host}/perfil/?u={handle_now}");
    let actor: Actor = Actor::person(
        &host,
        &handle_now,
        Some(ActorRole::Voter),
        profile.display_name,
    )
    .with_summary(summary_html)
    .with_url(Some(profile_url))
    .with_published(Some(profile.created_at.to_rfc3339()))
    .with_images(icon_url, image_url)
    .with_public_key(PublicKey::main_key(&actor_url, public_pem));
    // The Update activity wraps the whole Actor as its object.
    let activity = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor_url}/activities/update-{}", uuid::Uuid::now_v7()),
        "type": "Update",
        "actor": actor_url,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{actor_url}/followers")],
        "object": actor,
    });
    // Deliver directly (best-effort) to every ACK'd inbound follower.
    let inboxes: Vec<String> = match sqlx::query_scalar::<_, String>(
        r"SELECT remote_inbox_url FROM federation_follow
           WHERE citizen_id = $1
             AND direction = 'inbound'
             AND accepted_at IS NOT NULL
             AND remote_inbox_url IS NOT NULL",
    )
    .bind(caller.citizen.as_uuid())
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "failed to list follower inboxes");
            return server_error();
        }
    };
    let private_pem = match svc.read_actor_private_key(caller.citizen).await {
        Ok(pem) => pem,
        Err(_) => return server_error(),
    };
    let mut delivered = 0u64;
    for inbox in &inboxes {
        if deliver_signed(&actor_url, &private_pem, inbox, &activity)
            .await
            .is_ok()
        {
            delivered += 1;
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "delivered_to": delivered,
            "targets": inboxes.len(),
        }))),
    )
        .into_response()
}

/// `POST /api/v1/me/media` — multipart upload of a single image attachment.
/// Accepts `file` (bytes) and optional `alt_text`. Returns the persisted
/// media row + public URL so the composer can preview it and later reference
/// its `id` inside `POST /me/notes`. The body cap (10 MiB) is set on the
/// client-routes layer above.
async fn post_my_media(
    State(state): State<AppState>,
    caller: CallerId,
    mut multipart: axum::extract::Multipart,
) -> Response {
    // Read the two fields we care about: `file` (bytes) and `alt_text` (text).
    // Ignore anything else — front-end clients might send junk headers.
    let mut file: Option<Vec<u8>> = None;
    let mut alt: Option<String> = None;
    loop {
        let next = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "multipart parse failed");
                return client_error("upload inválido");
            }
        };
        match next.name() {
            Some("file") => match next.bytes().await {
                Ok(b) => file = Some(b.to_vec()),
                Err(_) => return client_error("falha ao ler o arquivo"),
            },
            Some("alt_text") => alt = next.text().await.ok(),
            _ => {
                let _ = next.bytes().await;
            }
        }
    }
    let Some(bytes) = file else {
        return client_error("envie o arquivo no campo `file`");
    };
    let svc = ProfileService::from_state(&state);
    let me = match svc
        .find_public_by_handle(caller.org, &handle_of(&svc, caller.citizen).await)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return client_error(
                "torne seu perfil público antes de enviar mídia (em Configurações)",
            );
        }
    };
    let po = public_origin();
    let me_url = format!(
        "{}/actors/{}",
        po.trim_end_matches('/'),
        me.handle.as_deref().unwrap_or(&me.public_handle)
    );
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match note_media::upload_image(
        &state.db,
        state.storage.as_ref(),
        &me_url,
        bytes,
        alt,
        &media_base,
    )
    .await
    {
        Ok(m) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({
                "id": m.id,
                "url": m.url,
                "kind": m.kind,
                "content_type": m.content_type,
                "alt_text": m.alt_text,
                "width": m.width,
                "height": m.height,
            }))),
        )
            .into_response(),
        Err(err) => {
            let msg = err.user_message();
            match err {
                note_media::UploadError::TooLarge
                | note_media::UploadError::NotAnImage
                | note_media::UploadError::StorageUnwired => client_error(&msg),
                _ => {
                    tracing::error!(error = ?err, "media upload failed");
                    server_error()
                }
            }
        }
    }
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
    match notifications::list_for_citizen(&state.db, caller.citizen.as_uuid(), limit, offset).await
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
async fn clear_my_notifications(State(state): State<AppState>, caller: CallerId) -> Response {
    match notifications::mark_all_read(&state.db, caller.citizen.as_uuid()).await {
        Ok(n) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "cleared": n }))),
        )
            .into_response(),
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
    match federation_feed::list_hashtag_timeline(&state.db, &normalized, &media_base, limit, offset)
        .await
    {
        Ok(mut items) => {
            federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
            federation_feed::enrich_with_polls(&state.db, &mut items, None).await;
            (
                StatusCode::OK,
                Json(ApiResponse::ok(json!({
                    "tag": normalized,
                    "items": items,
                }))),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "hashtag timeline query failed");
            server_error()
        }
    }
}

/// Query for `/search*` — the `q` is the free-text prefix. `limit` is
/// clamped in the handlers.
#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

/// Search page params for `/directory` and `/suggestions/follow`.
#[derive(Debug, Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `GET /api/v1/search/hashtags?q=` — prefix autocomplete over
/// `note_hashtag.tag_normalized`. Public (no auth).
async fn search_hashtags(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(8).clamp(1, 20);
    match discovery::hashtags_matching(&state.db, &query.q, limit).await {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "items": items }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "search hashtags failed");
            server_error()
        }
    }
}

/// `GET /api/v1/search/mentions?q=` — autocomplete local `@handle`. Public.
async fn search_mentions(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(8).clamp(1, 20);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match discovery::mentions_matching(&state.db, &query.q, &public_origin(), &media_base, limit)
        .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "items": items }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "search mentions failed");
            server_error()
        }
    }
}

/// `GET /api/v1/search?q=` — unified search: accounts + hashtags + notes.
async fn search_all(State(state): State<AppState>, Query(query): Query<SearchQuery>) -> Response {
    let per = query.limit.unwrap_or(10).clamp(1, 30);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let po = public_origin();
    let hashtags = discovery::hashtags_matching(&state.db, &query.q, per)
        .await
        .unwrap_or_default();
    let accounts = discovery::mentions_matching(&state.db, &query.q, &po, &media_base, per)
        .await
        .unwrap_or_default();
    let notes = discovery::notes_matching(&state.db, &query.q, &media_base, per)
        .await
        .unwrap_or_default();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "accounts": accounts,
            "hashtags": hashtags,
            "notes": notes,
        }))),
    )
        .into_response()
}

/// `GET /api/v1/trends/hashtags?window_hours=24&limit=10` — trending tags.
async fn trending_hashtags(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(10).clamp(1, 30);
    match discovery::trending_hashtags(&state.db, 24, limit).await {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "items": items }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "trending hashtags failed");
            server_error()
        }
    }
}

/// `GET /api/v1/directory?limit=&offset=` — profile directory.
async fn directory_endpoint(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(24).clamp(1, 60);
    let offset = query.offset.unwrap_or(0).max(0);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match discovery::directory(&state.db, &public_origin(), &media_base, limit, offset).await {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "items": items }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "directory query failed");
            server_error()
        }
    }
}

/// `GET /api/v1/suggestions/follow?limit=` — people the caller doesn't
/// follow yet, ordered by recent outbox activity. Auth required.
async fn follow_suggestions_endpoint(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(12).clamp(1, 30);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match discovery::follow_suggestions(
        &state.db,
        caller.citizen.as_uuid(),
        &public_origin(),
        &media_base,
        limit,
    )
    .await
    {
        Ok(items) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "items": items }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "follow suggestions failed");
            server_error()
        }
    }
}

/// Best-effort resolve the caller's canonical Actor URL. Used to scope
/// `voted_option_ids` on the feed's poll enrichment. Returns `None` when the
/// citizen has no public handle yet (they can't vote in that state anyway,
/// so an unscoped poll is fine).
async fn viewer_actor_url_of(state: &AppState, caller: &CallerId) -> Option<String> {
    let svc = ProfileService::from_state(state);
    let handle_now = handle_of(&svc, caller.citizen).await;
    if handle_now.starts_with("u-") || handle_now.is_empty() {
        return None;
    }
    let po = public_origin();
    Some(format!(
        "{}/actors/{}",
        po.trim_end_matches('/'),
        handle_now
    ))
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
        Ok(mut items) => {
            federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
            let viewer = viewer_actor_url_of(&state, &caller).await;
            federation_feed::enrich_with_polls(&state.db, &mut items, viewer.as_deref()).await;
            (StatusCode::OK, Json(ApiResponse::ok(items))).into_response()
        }
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
        Ok(mut items) => {
            federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
            let viewer = viewer_actor_url_of(&state, &caller).await;
            federation_feed::enrich_with_polls(&state.db, &mut items, viewer.as_deref()).await;
            (StatusCode::OK, Json(ApiResponse::ok(items))).into_response()
        }
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
    let existing = match federation_feed::find_local_reaction(
        &state.db,
        citizen,
        &object_uri,
        db_kind,
    )
    .await
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
        return Err(FederationError::Http(
            "remote author has no inbox".to_owned(),
        ));
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
    // Accept the Note if EITHER (a) at least one local citizen follows the sender, OR
    // (b) the Note mentions a local user, OR (c) the Note replies to one of our objects.
    // Mastodon-compatible: mentions and replies from strangers must always reach the
    // recipient's inbox — a stricter gate would silently drop first-contact mentions.
    let po_for_gate = public_origin();
    let mentions_us = object
        .get("tag")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter().any(|t| {
                let ttype = t.get("type").and_then(Value::as_str).unwrap_or("");
                let is_mention = ttype == "Mention" || ttype.ends_with(":Mention");
                if !is_mention {
                    return false;
                }
                let href = t.get("href").and_then(Value::as_str).unwrap_or("");
                let prefix = format!("{}/actors/", po_for_gate.trim_end_matches('/'));
                href.starts_with(&prefix)
            })
        })
        .unwrap_or(false);
    let replies_to_us = if let Some(reply_uri) = object.get("inReplyTo").and_then(Value::as_str) {
        federation_feed::is_our_object(&state.db, reply_uri)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    match federation_feed::anyone_follows(&state.db, signer_actor_url).await {
        Ok(true) => {}
        Ok(false) => {
            if !mentions_us && !replies_to_us {
                tracing::debug!(actor = %signer_actor_url, "Create(Note) from unfollowed actor, no local mention/reply — ignored");
                return;
            }
            tracing::info!(
                actor = %signer_actor_url,
                mentions_us,
                replies_to_us,
                "Create(Note) from unfollowed actor accepted — first-contact mention/reply"
            );
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
        if let Ok(Some(owner_id)) = notifications::find_owner_of_object(&state.db, reply_uri).await
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
            // JSON-LD serializers may use `Mention` (compact) or `as:Mention`
            // (namespace-prefixed) interchangeably. Normalize by stripping any
            // prefix so we accept both.
            let ttype_short = ttype.rsplit(':').next().unwrap_or(ttype);
            match ttype_short {
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
    // 0.18.0-rc1: inbound `attachment[]` (Mastodon/Pleroma/others send Document/Image
    // objects here). Persist each as a remote-only `media_attachment` row and bind
    // it to this Note via `note_media`. Types not recognized are skipped without error.
    if let Some(atts) = object.get("attachment").and_then(Value::as_array) {
        let mut order = 0i32;
        for att in atts {
            let att_type = att.get("type").and_then(Value::as_str).unwrap_or("");
            let att_short = att_type.rsplit(':').next().unwrap_or(att_type);
            if !matches!(att_short, "Image" | "Video" | "Audio" | "Document") {
                continue;
            }
            let Some(url) = att.get("url").and_then(Value::as_str).or_else(|| {
                // Some servers wrap the url inside a Link object; try `.url[0].href`.
                att.get("url")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|first| first.get("href").and_then(Value::as_str))
            }) else {
                continue;
            };
            let media_type = att
                .get("mediaType")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let alt = att.get("name").and_then(Value::as_str);
            let width = att.get("width").and_then(Value::as_i64).map(|v| v as i32);
            let height = att.get("height").and_then(Value::as_i64).map(|v| v as i32);
            match note_media::upsert_remote_media(
                &state.db,
                signer_actor_url,
                url,
                media_type,
                alt,
                width,
                height,
            )
            .await
            {
                Ok(media_id) => {
                    let _ = sqlx::query(
                        r"INSERT INTO note_media
                             (id, object_uri, media_id, sort_order, created_at)
                          VALUES ($1, $2, $3, $4, $5)
                          ON CONFLICT (object_uri, media_id) DO NOTHING",
                    )
                    .bind(uuid::Uuid::now_v7())
                    .bind(object_uri)
                    .bind(media_id)
                    .bind(order)
                    .bind(now)
                    .execute(&state.db)
                    .await;
                    order += 1;
                }
                Err(err) => {
                    tracing::debug!(error = ?err, "skipped remote attachment we couldn't classify");
                }
            }
        }
    }
    if !saw_tag_array {
        // Fallback for remotes that don't populate tag[] — extract from content.
        // Uses our own `public_origin` so local mentions resolve correctly.
        for m in dsoc_federation::extract_mentions(&content) {
            let actor_url_guess = m.best_actor_url(&po);
            let _ = federation_feed::upsert_mention(
                &state.db,
                object_uri,
                &actor_url_guess,
                &m.handle,
                now,
            )
            .await;
            // Also fire a notification if this fallback-mention hits a local user.
            if let Ok(Some(mentioned_id)) =
                notifications::find_local_citizen_by_actor_url(&state.db, &actor_url_guess, &po)
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
    let notif_kind = if kind == "Like" {
        "favourite"
    } else {
        "reblog"
    };
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
pub(crate) async fn handle_of(svc: &ProfileService, citizen: CitizenId) -> String {
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
    let name = actor.get("name").and_then(Value::as_str).map(str::to_owned);
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
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(&body))
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

/// Extract handle `@user@host` a partir de um actor URL — pra a UI ter algo
/// legível sem outro round-trip. Cobre `https://host/users/user`,
/// `https://host/@user` e `https://host/actors/user`.
fn hint_handle_from_actor_url(u: &str) -> Option<String> {
    let host = host_from_url(u)?;
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let path_start = rest.find('/')?;
    let path = &rest[path_start..];
    // Extrair último segmento do path
    let seg = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let seg = seg.strip_prefix('@').unwrap_or(seg);
    if seg.is_empty() {
        None
    } else {
        Some(format!("@{seg}@{host}"))
    }
}

#[derive(Debug, Serialize)]
struct SocialLinkDto {
    /// URL AP do actor remoto (opaca).
    actor_url: String,
    /// Handle inferido do URL — `@user@host` — pra UI. Pode não bater 100 %
    /// com o preferredUsername quando o site usa slug ≠ username, mas serve
    /// como âncora clicável.
    handle_hint: Option<String>,
    /// Timestamp do accepted_at (ou created_at pra pending).
    since: DateTime<Utc>,
    /// True se o Follow foi aceito pelo lado remoto.
    accepted: bool,
}

async fn my_following_list(State(state): State<AppState>, caller: CallerId) -> Response {
    social_list(&state, caller.citizen.as_uuid(), "outbound").await
}

async fn my_followers_list(State(state): State<AppState>, caller: CallerId) -> Response {
    social_list(&state, caller.citizen.as_uuid(), "inbound").await
}

#[derive(Debug, Deserialize)]
struct BulkFollowBody {
    /// Lista de actor URLs (`https://host/users/x` ou `https://host/actors/y`) ou
    /// handles `@user@host`. Handles são resolvidos via WebFinger.
    entries: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BulkFollowResult {
    total: usize,
    followed: usize,
    already: usize,
    failed: usize,
    errors: Vec<String>,
}

/// Recebe uma lista mista de handles/URLs e dispara Follow pra cada um.
/// Best-effort: cada falha vira uma string em `errors`. Cap em 200 por chamada
/// pra evitar abuso.
async fn bulk_follow(
    State(state): State<AppState>,
    caller: CallerId,
    AxumJson(body): AxumJson<BulkFollowBody>,
) -> Response {
    let entries: Vec<String> = body
        .entries
        .into_iter()
        .filter_map(|s| {
            let t = s.trim().to_owned();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
        .take(200)
        .collect();
    let mut result = BulkFollowResult {
        total: entries.len(),
        followed: 0,
        already: 0,
        failed: 0,
        errors: Vec::new(),
    };
    for entry in entries {
        // Resolve pra URL: se começa com http, é URL; se começa com @, é handle → webfinger.
        let actor_url = if entry.starts_with("https://") || entry.starts_with("http://") {
            entry.clone()
        } else {
            // Deriva via lookup_remote inline (reusa fetch_remote_actor + webfinger).
            let raw = entry.trim_start_matches('@');
            let Some((user, host)) = raw.rsplit_once('@') else {
                result.failed += 1;
                result.errors.push(format!("{entry}: formato inválido"));
                continue;
            };
            let webfinger_url =
                format!("https://{host}/.well-known/webfinger?resource=acct:{user}@{host}");
            let jrd = match fetch_remote_actor(&webfinger_url).await {
                Ok(v) => v,
                Err(_) => {
                    result.failed += 1;
                    result.errors.push(format!("{entry}: webfinger falhou"));
                    continue;
                }
            };
            let self_url = jrd
                .get("links")
                .and_then(Value::as_array)
                .and_then(|links| {
                    links.iter().find_map(|l| {
                        let rel = l.get("rel").and_then(Value::as_str)?;
                        let typ = l.get("type").and_then(Value::as_str)?;
                        if rel == "self" && typ.contains("activity") {
                            l.get("href").and_then(Value::as_str).map(str::to_owned)
                        } else {
                            None
                        }
                    })
                });
            match self_url {
                Some(url) => url,
                None => {
                    result.failed += 1;
                    result
                        .errors
                        .push(format!("{entry}: instância sem ActivityPub self link"));
                    continue;
                }
            }
        };
        // Verifica se já segue.
        let already: bool = sqlx::query_scalar::<_, bool>(
            r"SELECT EXISTS (SELECT 1 FROM federation_follow
                             WHERE citizen_id = $1 AND direction = 'outbound' AND remote_actor_url = $2)",
        )
        .bind(caller.citizen.as_uuid())
        .bind(&actor_url)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
        if already {
            result.already += 1;
            continue;
        }
        // Dispara o mesmo caminho do follow_remote via chamada interna helper.
        // Simplificado: só valida se resolve e insere pending row; delivery
        // real usa o worker. Pra minimizar duplicação, reusa a rota:
        match do_follow_remote(&state, caller, &actor_url).await {
            Ok(()) => result.followed += 1,
            Err(msg) => {
                result.failed += 1;
                result.errors.push(format!("{entry}: {msg}"));
            }
        }
    }
    (StatusCode::OK, Json(ApiResponse::ok(result))).into_response()
}

async fn do_follow_remote(
    state: &AppState,
    caller: CallerId,
    actor_url: &str,
) -> Result<(), String> {
    let svc = ProfileService::from_state(state);
    let me = svc
        .find_public_by_handle(caller.org, &handle_of(&svc, caller.citizen).await)
        .await
        .map_err(|_| "perfil não é público".to_string())?;
    let _ = svc
        .ensure_actor_public_key(caller.citizen)
        .await
        .map_err(|e| format!("chave: {e:?}"))?;
    let private_pem = svc
        .read_actor_private_key(caller.citizen)
        .await
        .map_err(|e| format!("chave privada: {e:?}"))?;
    let remote_actor = fetch_remote_actor(actor_url)
        .await
        .map_err(|_| "actor remoto não respondeu".to_string())?;
    let remote_inbox = remote_actor
        .get("inbox")
        .and_then(Value::as_str)
        .ok_or_else(|| "actor sem inbox".to_string())?
        .to_owned();
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
        "object": actor_url,
    });
    deliver_signed(&me_url, &private_pem, &remote_inbox, &follow)
        .await
        .map_err(|e| format!("entrega: {e:?}"))?;
    // Persiste outbound pending.
    let _ = sqlx::query(
        r"INSERT INTO federation_follow
            (id, citizen_id, direction, remote_actor_url, remote_inbox_url,
             activity_id, created_at)
          VALUES ($1, $2, 'outbound', $3, $4, $5, now())
          ON CONFLICT (citizen_id, direction, remote_actor_url) DO NOTHING",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(caller.citizen.as_uuid())
    .bind(actor_url)
    .bind(&remote_inbox)
    .bind(&activity_id)
    .execute(&state.db)
    .await;
    Ok(())
}

async fn social_list(state: &AppState, citizen: uuid::Uuid, direction: &str) -> Response {
    let rows = sqlx::query_as::<_, (String, Option<DateTime<Utc>>, DateTime<Utc>)>(
        r"SELECT remote_actor_url, accepted_at, created_at
            FROM federation_follow
           WHERE citizen_id = $1 AND direction = $2
           ORDER BY COALESCE(accepted_at, created_at) DESC
           LIMIT 500",
    )
    .bind(citizen)
    .bind(direction)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<SocialLinkDto> = rows
                .into_iter()
                .map(|(actor_url, accepted_at, created_at)| SocialLinkDto {
                    handle_hint: hint_handle_from_actor_url(&actor_url),
                    since: accepted_at.unwrap_or(created_at),
                    accepted: accepted_at.is_some(),
                    actor_url,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "social_list");
            server_error()
        }
    }
}

/// Extract the host from `https://host[:port]/…` sem depender do crate `url`.
/// Retorna None se não for uma URL http(s) reconhecível.
fn host_from_url(u: &str) -> Option<String> {
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let end = rest.find('/').unwrap_or(rest.len());
    let hostport = &rest[..end];
    let host = hostport
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Escape as 5 chars perigosos pra qualquer atributo/texto HTML.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encode pra query string (subconjunto reservado do RFC 3986).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Colapsa qualquer sequência de espaços/quebras + remove tags — bom o suficiente pra
/// snippet de OG description sem trazer sanitizer de HTML no runtime.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut last_ws = false;
    for c in s.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
            continue;
        }
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c.is_whitespace() {
            if !last_ws && !out.is_empty() {
                out.push(' ');
                last_ws = true;
            }
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out.trim().to_owned()
}

/// Trunca a `max` chars mantendo unicode grapheme rough; adiciona reticência quando corta.
fn truncate_chars(s: &str, max: usize) -> String {
    let mut end = s.len();
    for (count, (i, _)) in s.char_indices().enumerate() {
        if count == max {
            end = i;
            break;
        }
    }
    if end == s.len() {
        s.to_owned()
    } else {
        format!("{}…", &s[..end])
    }
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
