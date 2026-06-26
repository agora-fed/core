//! # Gateway-level ActivityPub surface — composes auth (DB-backed identity) with the federation
//! crate (pure AP builders). ADR-0010 W2.
//!
//! The federation crate is Tier-3 and may not touch the DB; the gateway is the composition root
//! that pulls citizen identity from `dsoc-auth::ProfileService`, materializes the keypair (lazy),
//! and hands the data to `dsoc-federation`'s pure builders. This split keeps federation
//! transportable to a future hub instance without dragging the platform's DB with it.
//!
//! Routes (mounted at the root, NOT under `/api/v1`, so federation paths look like every other
//! ActivityPub instance: `/.well-known/webfinger`, `/actors/<handle>`):
//!
//! * `GET /.well-known/webfinger?resource=acct:<handle>@<host>` — RFC 7033 JRD pointing at the
//!   Actor document. Returns **404** for unknown handles AND for citizens who have not opted
//!   into a public profile (LGPD: the public surface never confirms a private account exists).
//! * `GET /actors/{handle}` — the ActivityStreams Actor `Person` document, including the
//!   `publicKey` used by HTTP Signatures verifiers. The keypair is generated on first read for
//!   public citizens that do not have one yet (one-time ~100ms RSA-2048 generation; subsequent
//!   reads are a plain SELECT).

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_app::AppState;
use dsoc_auth::profile::ProfileService;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_federation::signatures::PublicKey;
use dsoc_federation::{actor_id, Actor, ActorRole};
use serde::Deserialize;

const ACTIVITY_JSON: &str = "application/activity+json";
const JRD_JSON: &str = "application/jrd+json";

/// The tenant whose handles this instance serves. Single-tenant for now (the seeded
/// `DemocraciaBR` org); when the platform goes multi-tenant the resolution becomes host-based.
const DEFAULT_ORG_UUID: uuid::Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Mount the federation HTTP surface on the gateway's root router.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/actors/{handle}", get(actor_handler))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct WebFingerQuery {
    /// The `acct:user@host` resource being resolved.
    resource: String,
}

/// `GET /.well-known/webfinger` — resolves `acct:<handle>@<host>` to the actor self-link, only
/// for citizens whose `is_public = true` (private accounts are invisible to federation discovery).
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
        // An instance only answers WebFinger for its own host (RFC 7033 § 8.2).
        return StatusCode::NOT_FOUND.into_response();
    }

    // Resolve the citizen by user-chosen handle. Unknown / private → 404; the surface is
    // deliberately opaque about which it was.
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    if svc.find_public_by_handle(org, user).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Build the JRD that points at the citizen's actor URL.
    let actor_url = actor_id(&host, user);
    let jrd = serde_json::json!({
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

/// `GET /actors/{handle}` — the AP Actor `Person` for a public citizen. Lazy-generates the
/// keypair on first call so we don't pay RSA-2048 for citizens who never federate.
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

    let profile = match svc.find_public_by_handle(org, &handle).await {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    // Lazy keypair: first read for a freshly-public citizen takes ~100ms; subsequent reads are
    // a plain SELECT. The private PEM stays in the DB and never lands in the Actor document.
    let public_pem = match svc
        .ensure_actor_public_key(CitizenId::from_uuid(profile.citizen_id))
        .await
    {
        Ok(pem) => pem,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let actor_url = actor_id(&host, &handle);
    // `PublicKey::main_key` appends `#main-key` itself, so we pass the bare actor URL — passing
    // the suffixed key id would produce `...#main-key#main-key`.
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

/// Read the request authority from the `Host` header. Required (no fallback) — RFC 7033
/// resolution is tied to the host the client used.
fn host_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}
