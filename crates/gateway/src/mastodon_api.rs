//! # Mastodon Client API surface (0.19.0).
//!
//! Routes third-party clients (Ivory, Elk, Ice Cubes, Tusky, custom scripts)
//! expect to hit under `/api/v1/*` and `/oauth/*`. This crate is a
//! translation layer: no new persistence beyond `mastodon_oauth`; everything
//! else re-uses `federation_feed`, `notifications`, `note_media` and so on.
//!
//! Auth: the router accepts EITHER the cookie session (for our own web) OR
//! `Authorization: Bearer <token>`. The gateway's `inject_identity`
//! middleware already treats bearer as an alternate credential and stamps
//! the same `x-dsoc-citizen-id` / `x-dsoc-org-id` headers a cookie would.
//!
//! Response shape: **flat** Mastodon JSON (no `ApiResponse` envelope). The
//! `masto_json` helper below serializes any DTO directly.

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::{CitizenId, OrgId};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::federation_feed;
use crate::mastodon_dto::{Account, AccountBuild, Instance, Status};
use crate::mastodon_oauth::{
    self, exchange_authorization_code, exchange_password, issue_access_token,
    register_application, resolve_bearer, revoke_bearer, OAuthError, TokenPayload,
};

/// `/api/v1/apps` + `/api/v1/instance` + `/api/v1/accounts/*` +
/// `/api/v1/timelines/*` + `/api/v1/notifications` etc. Mounted UNDER the
/// same `/api/v1` prefix as our own client routes; the two sets don't
/// overlap.
pub fn masto_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/apps", post(post_apps))
        .route("/instance", get(get_instance_v1))
        .route("/accounts/verify_credentials", get(verify_credentials))
        .route("/timelines/home", get(get_timeline_home))
        .route("/timelines/public", get(get_timeline_public))
        .with_state(state)
}

/// The OAuth endpoints live at the ROOT — NOT under /api/v1 — per Mastodon
/// spec. Mounted alongside the well-known federation surface.
pub fn oauth_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/oauth/token", post(oauth_token))
        .route("/oauth/revoke", post(oauth_revoke))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// POST /api/v1/apps — register a client (public)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RegisterApp {
    client_name: String,
    /// Space-separated URIs per Mastodon convention; single URI also accepted.
    redirect_uris: String,
    #[serde(default)]
    scopes: Option<String>,
    #[serde(default)]
    website: Option<String>,
}

/// Register an OAuth application. Public — no auth required, matches
/// Mastodon behaviour. Returns `{client_id, client_secret, ...}`.
async fn post_apps(State(state): State<AppState>, Form(body): Form<RegisterApp>) -> Response {
    let uris: Vec<String> = body
        .redirect_uris
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if uris.is_empty() {
        return oauth_error(StatusCode::BAD_REQUEST, OAuthError::InvalidRequest("redirect_uris required"));
    }
    let scopes = body.scopes.as_deref().unwrap_or("read");
    match register_application(
        &state.db,
        &body.client_name,
        uris,
        scopes,
        body.website.as_deref(),
    )
    .await
    {
        Ok(app) => (
            StatusCode::OK,
            Json(json!({
                "id": app.id.to_string(),
                "name": app.name,
                "website": app.website,
                "redirect_uri": app.redirect_uris.first().cloned().unwrap_or_default(),
                "redirect_uris": app.redirect_uris,
                "client_id": app.client_id,
                "client_secret": app.client_secret,
                "scopes": app.scopes.split_whitespace().collect::<Vec<_>>(),
                "vapid_key": app.vapid_key,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "app registration failed");
            oauth_error(StatusCode::INTERNAL_SERVER_ERROR, OAuthError::ServerError)
        }
    }
}

// ---------------------------------------------------------------------------
// POST /oauth/token — exchange creds/code for an access token
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrantType {
    Password,
    AuthorizationCode,
    ClientCredentials,
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: GrantType,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    // password grant
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    // authorization_code grant
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
}

/// Body-form encoded token exchange. Supports:
/// * password grant (username=email, password=…)
/// * authorization_code grant (code=…, redirect_uri=…)
/// * client_credentials grant (app-only, no citizen scope)
async fn oauth_token(
    State(state): State<AppState>,
    Form(body): Form<TokenRequest>,
) -> Response {
    let client_id = body.client_id.as_deref().unwrap_or_default();
    let client_secret = body.client_secret.as_deref().unwrap_or_default();
    match body.grant_type {
        GrantType::Password => {
            let email = body.username.as_deref().unwrap_or_default();
            let password = body.password.as_deref().unwrap_or_default();
            if email.is_empty() || password.is_empty() {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    OAuthError::InvalidRequest("username and password required"),
                );
            }
            // Resolve email → citizen via the sovereign auth service.
            let svc = dsoc_auth::zitadel_from_state(&state);
            let default_org = OrgId::from_uuid(default_org_uuid());
            let session = match svc.login(default_org, email, password).await {
                Ok(s) => s,
                Err(_) => {
                    return oauth_error(StatusCode::UNAUTHORIZED, OAuthError::InvalidCredentials)
                }
            };
            let scopes = normalize_scope(body.scope.as_deref());
            match exchange_password(
                &state.db,
                client_id,
                client_secret,
                session.citizen.as_uuid(),
                &scopes,
            )
            .await
            {
                Ok(tok) => token_ok(tok),
                Err(err) => oauth_error(oauth_status(&err), err),
            }
        }
        GrantType::AuthorizationCode => {
            let code = body.code.as_deref().unwrap_or_default();
            let redirect = body.redirect_uri.as_deref().unwrap_or_default();
            if code.is_empty() || redirect.is_empty() {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    OAuthError::InvalidRequest("code and redirect_uri required"),
                );
            }
            match exchange_authorization_code(
                &state.db,
                client_id,
                client_secret,
                code,
                redirect,
            )
            .await
            {
                Ok(tok) => token_ok(tok),
                Err(err) => oauth_error(oauth_status(&err), err),
            }
        }
        GrantType::ClientCredentials => {
            // App-only token; no citizen.
            let app = match mastodon_oauth::find_application_by_client_id(&state.db, client_id)
                .await
            {
                Ok(Some(v)) => v,
                _ => return oauth_error(StatusCode::UNAUTHORIZED, OAuthError::InvalidClient),
            };
            if !mastodon_oauth::verify_client_secret(client_secret, &app.1) {
                return oauth_error(StatusCode::UNAUTHORIZED, OAuthError::InvalidClient);
            }
            let scopes = normalize_scope(body.scope.as_deref());
            match issue_access_token(&state.db, app.0, None, &scopes).await {
                Ok(tok) => token_ok(tok),
                Err(err) => {
                    tracing::error!(error = ?err, "client_credentials failed");
                    oauth_error(StatusCode::INTERNAL_SERVER_ERROR, OAuthError::ServerError)
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct RevokeRequest {
    token: String,
}

async fn oauth_revoke(
    State(state): State<AppState>,
    Form(body): Form<RevokeRequest>,
) -> Response {
    let _ = revoke_bearer(&state.db, body.token.trim()).await;
    // Mastodon returns 200 with an empty JSON object.
    (StatusCode::OK, Json(json!({}))).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/instance — Mastodon v1 instance metadata (public)
// ---------------------------------------------------------------------------

async fn get_instance_v1(State(state): State<AppState>) -> Response {
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    // Cheap DB stats — never block > 200ms.
    let (users, statuses): (i64, i64) = sqlx::query_as::<_, (i64, i64)>(
        r"SELECT
            (SELECT count(*) FROM citizen WHERE is_public = true) AS users,
            (SELECT count(*) FROM federation_outbox_entry WHERE kind = 'Create' AND deleted_at IS NULL) AS statuses",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));
    let inst = Instance::build(&host, users, statuses, None);
    (StatusCode::OK, Json(inst)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/accounts/verify_credentials
// ---------------------------------------------------------------------------

async fn verify_credentials(State(state): State<AppState>, caller: CallerId) -> Response {
    match build_account_for_citizen(&state, caller.citizen).await {
        Some(a) => (StatusCode::OK, Json(a)).into_response(),
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

async fn build_account_for_citizen(
    state: &AppState,
    citizen: CitizenId,
) -> Option<Account> {
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    // Query the citizen row directly to avoid another network hop.
    let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            r"SELECT
                handle,
                display_name,
                bio,
                CASE WHEN avatar_object_key IS NOT NULL AND $2 <> ''
                     THEN $2 || '/' || avatar_object_key END,
                CASE WHEN cover_object_key IS NOT NULL AND $2 <> ''
                     THEN $2 || '/' || cover_object_key END,
                created_at
              FROM citizen
             WHERE id = $1
               AND handle IS NOT NULL",
        )
        .bind(citizen.as_uuid())
        .bind(media_base.trim_end_matches('/'))
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let (handle, display_name, bio, avatar, cover, created_at) = row?;
    // Counts — cheap enough for verify_credentials.
    let followers_count: i64 = sqlx::query_scalar(
        r"SELECT count(*) FROM federation_follow WHERE citizen_id = $1 AND direction = 'inbound' AND accepted_at IS NOT NULL",
    )
    .bind(citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let following_count: i64 = sqlx::query_scalar(
        r"SELECT count(*) FROM federation_follow WHERE citizen_id = $1 AND direction = 'outbound'",
    )
    .bind(citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let statuses_count: i64 = sqlx::query_scalar(
        r"SELECT count(*) FROM federation_outbox_entry WHERE citizen_id = $1 AND kind = 'Create' AND deleted_at IS NULL",
    )
    .bind(citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let last_status_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        r"SELECT max(created_at) FROM federation_outbox_entry WHERE citizen_id = $1 AND kind = 'Create' AND deleted_at IS NULL",
    )
    .bind(citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .unwrap_or(None);
    let bio_html = bio
        .as_deref()
        .map(dsoc_federation::plain_bio_to_html)
        .filter(|s| !s.is_empty());
    Some(Account::from_local(AccountBuild {
        citizen_id_str: citizen.as_uuid().to_string(),
        handle: &handle,
        display_name: display_name.as_deref(),
        bio_html,
        avatar_url: avatar.as_deref(),
        cover_url: cover.as_deref(),
        created_at,
        host: &host,
        followers_count,
        following_count,
        statuses_count,
        last_status_at,
    }))
}

// ---------------------------------------------------------------------------
// GET /api/v1/timelines/{home,public}
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TimelineQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    max_id: Option<String>,
    #[serde(default)]
    since_id: Option<String>,
    #[serde(default)]
    min_id: Option<String>,
}

async fn get_timeline_home(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<TimelineQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 40);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let public_origin = format!("https://{host}");
    let mut items = match federation_feed::list_feed(
        &state.db,
        caller.citizen.as_uuid(),
        &public_origin,
        &media_base,
        limit,
        0,
    )
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "masto home feed failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({}))).into_response();
        }
    };
    federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
    // Poll enrichment scoped to the viewer.
    let viewer = build_account_for_citizen(&state, caller.citizen)
        .await
        .map(|a| a.uri);
    federation_feed::enrich_with_polls(&state.db, &mut items, viewer.as_deref()).await;
    // Convert FeedItemDto -> Status.
    let out: Vec<Status> = items
        .into_iter()
        .map(|it| {
            let account = Account::from_remote_stub(
                &it.author_handle.trim_start_matches('@').to_owned(),
                it.author_display_name.as_deref(),
                it.author_avatar_url.as_deref(),
                &it.object_uri,
            );
            Status::from_feed_item(&it, account)
        })
        .collect();
    (StatusCode::OK, Json(out)).into_response()
}

async fn get_timeline_public(
    State(state): State<AppState>,
    Query(query): Query<TimelineQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 40);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    // Very simple approximation of Mastodon's public timeline: all
    // non-deleted, non-followers-only local Create(Note) rows, newest first.
    let rows: Vec<(String, String, Option<String>, Option<String>, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            r"SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS object_uri,
                     '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', '')) AS author_handle,
                     c.display_name,
                     CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                          THEN $2 || '/' || c.avatar_object_key END,
                     COALESCE(oe.payload->'object'->>'content', '') AS content_html,
                     oe.created_at
                FROM federation_outbox_entry oe
                JOIN citizen c ON c.id = oe.citizen_id
               WHERE oe.kind = 'Create'
                 AND oe.deleted_at IS NULL
                 AND oe.visibility = 'public'
               ORDER BY oe.created_at DESC
               LIMIT $1",
        )
        .bind(limit)
        .bind(media_base.trim_end_matches('/'))
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(object_uri, handle, display_name, avatar, content, created_at)| {
                let handle_clean = handle.trim_start_matches('@').to_owned();
                let actor_url = format!("https://{host}/actors/{handle_clean}");
                let account = Account::from_remote_stub(
                    &handle_clean,
                    display_name.as_deref(),
                    avatar.as_deref(),
                    &actor_url,
                );
                // Minimal Status — public timeline doesn't need full enrichment.
                json!({
                    "id": crate::mastodon_dto::short_hash(&object_uri),
                    "uri": object_uri,
                    "url": object_uri,
                    "account": account,
                    "content": content,
                    "created_at": created_at.to_rfc3339(),
                    "sensitive": false,
                    "spoiler_text": "",
                    "visibility": "public",
                    "media_attachments": [],
                    "mentions": [],
                    "tags": [],
                    "emojis": [],
                    "favourites_count": 0,
                    "reblogs_count": 0,
                    "replies_count": 0,
                    "favourited": false,
                    "reblogged": false,
                    "muted": false,
                    "bookmarked": false,
                    "pinned": false,
                    "language": null,
                    "poll": null,
                    "card": null,
                    "application": null,
                })
            },
        )
        .collect();
    let _ = query.max_id; // pagination reserved for later
    let _ = query.since_id;
    let _ = query.min_id;
    (StatusCode::OK, Json(out)).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_org_uuid() -> Uuid {
    Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("static uuid")
}

fn normalize_scope(s: Option<&str>) -> String {
    let base = s.unwrap_or("read").to_lowercase();
    let mut parts: Vec<&str> = base
        .split_whitespace()
        .filter(|s| matches!(*s, "read" | "write" | "follow" | "push"))
        .collect();
    parts.sort();
    parts.dedup();
    if parts.is_empty() {
        "read".to_owned()
    } else {
        parts.join(" ")
    }
}

fn token_ok(tok: TokenPayload) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "access_token": tok.access_token,
            "token_type": tok.token_type,
            "scope": tok.scope,
            "created_at": tok.created_at,
        })),
    )
        .into_response()
}

fn oauth_status(err: &OAuthError) -> StatusCode {
    match err {
        OAuthError::InvalidClient | OAuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        OAuthError::InvalidGrant | OAuthError::UnsupportedGrant => StatusCode::BAD_REQUEST,
        OAuthError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
        OAuthError::ServerError => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn oauth_error(status: StatusCode, err: OAuthError) -> Response {
    (
        status,
        Json(json!({
            "error": err.code(),
            "error_description": err.description(),
        })),
    )
        .into_response()
}

/// Middleware helper: if the request carries a valid Bearer token AND no
/// cookie session was set, resolve it and inject the same identity headers
/// `inject_identity` uses. Callable from lib.rs's middleware chain.
pub async fn resolve_bearer_to_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<(Uuid, Uuid)> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let token = token.trim();
    let resolved = resolve_bearer(&state.db, token).await.ok()??;
    let citizen_id = resolved.citizen_id?;
    let org_id: Uuid = sqlx::query_scalar(r"SELECT org_id FROM citizen WHERE id = $1")
        .bind(citizen_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    Some((citizen_id, org_id))
}
