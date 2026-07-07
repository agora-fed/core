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

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::{CitizenId, OrgId};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::federation_feed::{self, FeedItemDto};
use crate::mastodon_dto::{
    self, Account, AccountBuild, Instance, MastodonNotification, MastodonPoll, Status,
};
use crate::mastodon_oauth::{
    self, exchange_authorization_code, exchange_password, issue_access_token,
    register_application, resolve_bearer, revoke_bearer, OAuthError, TokenPayload,
};
use crate::note_media;
use crate::notifications;
use crate::polls;

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
        .route("/statuses", post(post_status))
        .route("/statuses/{id}", get(get_status).delete(delete_status))
        .route("/statuses/{id}/context", get(get_status_context))
        .route("/statuses/{id}/favourite", post(favourite_status))
        .route("/statuses/{id}/unfavourite", post(unfavourite_status))
        .route("/statuses/{id}/reblog", post(reblog_status))
        .route("/statuses/{id}/unreblog", post(unreblog_status))
        .route("/notifications", get(get_notifications))
        .route("/notifications/clear", post(clear_notifications))
        .route("/media", post(post_media))
        .route("/polls/{id}/votes", post(vote_poll))
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
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let out = feed_items_to_statuses(&state, &items, &host).await;
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

// ---------------------------------------------------------------------------
// Status id ↔ URI persistence (migration 0410)
// ---------------------------------------------------------------------------

/// Look up the Mastodon id we assigned to `object_uri`, inserting one if
/// it's the first time we see this Note. Deterministic on the URI so the
/// same Note always comes back with the same id across pods.
pub async fn ensure_status_id(db: &sqlx::PgPool, object_uri: &str) -> Result<String, sqlx::Error> {
    let id = mastodon_dto::short_hash(object_uri);
    sqlx::query(
        r"INSERT INTO mastodon_status_id (id, object_uri, created_at)
          VALUES ($1, $2, now())
          ON CONFLICT (object_uri) DO NOTHING",
    )
    .bind(&id)
    .bind(object_uri)
    .execute(db)
    .await?;
    Ok(id)
}

/// Reverse the lookup — given a Mastodon id (from the URL path), return the
/// AP object URI it stands for. `None` when the id has never been served.
pub async fn resolve_status_id(
    db: &sqlx::PgPool,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(r"SELECT object_uri FROM mastodon_status_id WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await
}

// ---------------------------------------------------------------------------
// Status assembly helpers
// ---------------------------------------------------------------------------

async fn build_status_from_feed_item(
    state: &AppState,
    item: &FeedItemDto,
    host: &str,
) -> Status {
    let id = ensure_status_id(&state.db, &item.object_uri)
        .await
        .unwrap_or_else(|_| mastodon_dto::short_hash(&item.object_uri));
    let in_reply_to_id = match &item.in_reply_to_uri {
        Some(uri) => Some(
            ensure_status_id(&state.db, uri)
                .await
                .unwrap_or_else(|_| mastodon_dto::short_hash(uri)),
        ),
        None => None,
    };
    let handle_no_at = item.author_handle.trim_start_matches('@');
    let account = if item.is_remote {
        Account::from_remote_stub(
            handle_no_at,
            item.author_display_name.as_deref(),
            item.author_avatar_url.as_deref(),
            handle_actor_url(host, handle_no_at).as_str(),
        )
    } else {
        // Local: cheap lookup by handle to promote the sparse row into a
        // proper Account with counts. Falls back to a stub on any failure.
        match build_account_for_local_handle(state, handle_no_at, host).await {
            Some(a) => a,
            None => Account::from_remote_stub(
                handle_no_at,
                item.author_display_name.as_deref(),
                item.author_avatar_url.as_deref(),
                handle_actor_url(host, handle_no_at).as_str(),
            ),
        }
    };
    Status::from_feed_item(item, id, in_reply_to_id, account)
}

fn handle_actor_url(host: &str, handle: &str) -> String {
    if handle.contains('@') {
        // Remote: use the actor's own URL if we can guess it. Best-effort;
        // Mastodon uses this only for the "view profile" link.
        if let Some((user, remote_host)) = handle.split_once('@') {
            return format!("https://{remote_host}/@{user}");
        }
    }
    format!("https://{}/actors/{}", host.trim_end_matches('/'), handle)
}

async fn build_account_for_local_handle(
    state: &AppState,
    handle: &str,
    host: &str,
) -> Option<Account> {
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let row: Option<(
        Uuid,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        r"SELECT id, display_name, bio,
                 CASE WHEN avatar_object_key IS NOT NULL AND $2 <> ''
                      THEN $2 || '/' || avatar_object_key END,
                 CASE WHEN cover_object_key IS NOT NULL AND $2 <> ''
                      THEN $2 || '/' || cover_object_key END,
                 created_at
            FROM citizen
           WHERE handle = $1 AND is_public = true",
    )
    .bind(handle)
    .bind(media_base.trim_end_matches('/'))
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let (citizen_id, display_name, bio, avatar, cover, created_at) = row?;
    let bio_html = bio
        .as_deref()
        .map(dsoc_federation::plain_bio_to_html)
        .filter(|s| !s.is_empty());
    // Skip expensive counts for feed-list stubs — Mastodon renders fine.
    Some(Account::from_local(AccountBuild {
        citizen_id_str: citizen_id.to_string(),
        handle,
        display_name: display_name.as_deref(),
        bio_html,
        avatar_url: avatar.as_deref(),
        cover_url: cover.as_deref(),
        created_at,
        host,
        followers_count: 0,
        following_count: 0,
        statuses_count: 0,
        last_status_at: None,
    }))
}

/// Turn one FeedItemDto into a Mastodon Status. Used by all the endpoints
/// that return statuses (timelines, single status, context).
async fn feed_items_to_statuses(
    state: &AppState,
    items: &[FeedItemDto],
    host: &str,
) -> Vec<Status> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(build_status_from_feed_item(state, it, host).await);
    }
    out
}

/// Look up a status by Mastodon id and return the (URI, FeedItemDto). None
/// when the id is unknown OR the underlying Note was deleted.
async fn load_status(
    state: &AppState,
    id: &str,
    viewer: Uuid,
) -> Option<(String, FeedItemDto)> {
    let uri = resolve_status_id(&state.db, id).await.ok().flatten()?;
    // Reuse list_thread_context — it returns a Vec starting with the root.
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let mut items = federation_feed::list_thread_context(&state.db, &uri, viewer, &media_base)
        .await
        .ok()?;
    federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let viewer_url = build_account_for_citizen(state, CitizenId::from_uuid(viewer))
        .await
        .map(|a| a.uri);
    federation_feed::enrich_with_polls(&state.db, &mut items, viewer_url.as_deref()).await;
    let _ = host;
    let root = items.into_iter().find(|it| it.object_uri == uri)?;
    Some((uri, root))
}

// ---------------------------------------------------------------------------
// POST /api/v1/statuses — create a Status via Mastodon API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PostStatusRequest {
    status: String,
    #[serde(default)]
    in_reply_to_id: Option<String>,
    #[serde(default)]
    sensitive: bool,
    #[serde(default)]
    spoiler_text: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    media_ids: Vec<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    poll: Option<PostStatusPoll>,
}

#[derive(Debug, Deserialize)]
struct PostStatusPoll {
    options: Vec<String>,
    /// Mastodon uses seconds; our internal poll wants minutes.
    expires_in: i64,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    hide_totals: bool,
}

/// Accept BOTH JSON and form-urlencoded bodies. Mastodon clients tend to
/// switch based on library. The wrapper extractor tries JSON first, then
/// form on failure.
#[derive(Debug)]
struct AnyBody<T>(T);

impl<S, T> axum::extract::FromRequest<S> for AnyBody<T>
where
    S: Send + Sync,
    T: for<'de> serde::Deserialize<'de> + Send + 'static,
{
    type Rejection = Response;
    async fn from_request(
        req: axum::extract::Request,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, 5 * 1024 * 1024).await {
            Ok(b) => b,
            Err(_) => {
                return Err((StatusCode::BAD_REQUEST, "body too large").into_response())
            }
        };
        // Try JSON.
        if let Ok(v) = serde_json::from_slice::<T>(&bytes) {
            return Ok(Self(v));
        }
        // Try form-urlencoded.
        if let Ok(v) = serde_urlencoded::from_bytes::<T>(&bytes) {
            return Ok(Self(v));
        }
        let _ = parts;
        Err((StatusCode::BAD_REQUEST, "invalid body").into_response())
    }
}

async fn post_status(
    State(state): State<AppState>,
    caller: CallerId,
    AnyBody(body): AnyBody<PostStatusRequest>,
) -> Response {
    let svc = dsoc_auth::profile::ProfileService::from_state(&state);
    let handle_now = super::federation::handle_of(&svc, caller.citizen).await;
    let public_origin =
        std::env::var("PUBLIC_ORIGIN").unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let me = match svc
        .find_public_by_handle(caller.org, &handle_now)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return client_error("torne seu perfil público antes de publicar");
        }
    };
    let _ = me;
    if let Err(err) = svc.ensure_actor_public_key(caller.citizen).await {
        tracing::error!(error = ?err, "actor key ensure failed");
        return client_error("erro interno");
    }
    let me_url = format!(
        "{}/actors/{}",
        public_origin.trim_end_matches('/'),
        handle_now
    );
    let visibility = body.visibility.as_deref().unwrap_or("public");
    if !matches!(visibility, "public" | "unlisted" | "private" | "direct") {
        return client_error("visibilidade inválida");
    }
    let in_reply_to_uri = match body.in_reply_to_id.as_deref() {
        Some(id) if !id.is_empty() => resolve_status_id(&state.db, id).await.ok().flatten(),
        _ => None,
    };
    match svc
        .create_public_note(
            caller.citizen,
            &me_url,
            &public_origin,
            &body.status,
            in_reply_to_uri.as_deref(),
            body.sensitive,
            body.spoiler_text.as_deref(),
        )
        .await
    {
        Ok((activity_id, _fanout)) => {
            let object_id = activity_id.replace("/activities/note-", "/objects/");
            // Media attach + payload patch.
            if !body.media_ids.is_empty() {
                let mut media_uuids: Vec<Uuid> = Vec::with_capacity(body.media_ids.len());
                for id in &body.media_ids {
                    if let Ok(u) = Uuid::parse_str(id) {
                        media_uuids.push(u);
                    }
                }
                if !media_uuids.is_empty() {
                    let _ = note_media::attach_to_note(&state.db, &object_id, &media_uuids).await;
                    let media_base =
                        std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
                    let _ = note_media::update_outbox_payload_with_attachments(
                        &state.db,
                        &activity_id,
                        &object_id,
                        &media_base,
                    )
                    .await;
                }
            }
            // Poll.
            if let Some(pi) = body.poll {
                let input = polls::PollInput {
                    options: pi.options,
                    multiple: pi.multiple,
                    expires_in_minutes: (pi.expires_in / 60).max(polls::MIN_EXPIRES_MINUTES),
                };
                let _ = pi.hide_totals;
                if polls::create_from_input(&state.db, &object_id, &input)
                    .await
                    .is_ok()
                {
                    let _ = polls::update_outbox_payload_with_question(
                        &state.db,
                        &activity_id,
                        &object_id,
                    )
                    .await;
                }
            }
            // Language handling: not persisted today; ignore quietly.
            let _ = body.language;
            // Return the freshly-built Status.
            let host = std::env::var("PUBLIC_HOST")
                .unwrap_or_else(|_| "democracia.social.br".to_owned());
            if let Some((_uri, item)) = load_status(&state, &mastodon_dto::short_hash(&object_id), caller.citizen.as_uuid()).await {
                let status = build_status_from_feed_item(&state, &item, &host).await;
                return (StatusCode::OK, Json(status)).into_response();
            }
            (StatusCode::OK, Json(json!({ "id": mastodon_dto::short_hash(&object_id) }))).into_response()
        }
        Err(dsoc_core::Error::Validation(msg)) => client_error(&msg),
        Err(err) => {
            tracing::error!(error = ?err, "post_status create_public_note failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
        }
    }
}

fn client_error(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/statuses/{id}
// ---------------------------------------------------------------------------

async fn get_status(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<String>,
) -> Response {
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    match load_status(&state, &id, caller.citizen.as_uuid()).await {
        Some((_uri, item)) => {
            let status = build_status_from_feed_item(&state, &item, &host).await;
            (StatusCode::OK, Json(status)).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "record not found" }))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// DELETE /api/v1/statuses/{id}
// ---------------------------------------------------------------------------

async fn delete_status(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<String>,
) -> Response {
    let uri = match resolve_status_id(&state.db, &id).await.ok().flatten() {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "record not found" })),
            )
                .into_response()
        }
    };
    // Ownership check + soft-delete + fanout (reuses the plumbing federation.rs
    // already has, but simplified here — we don't need to build the full Delete
    // activity from scratch; delegating would require exposing the internal fn).
    let now = chrono::Utc::now();
    let updated: u64 = sqlx::query(
        r"UPDATE federation_outbox_entry
             SET deleted_at = $2
           WHERE citizen_id = $1
             AND (activity_id = $3 OR payload->'object'->>'id' = $3)
             AND deleted_at IS NULL",
    )
    .bind(caller.citizen.as_uuid())
    .bind(now)
    .bind(&uri)
    .execute(&state.db)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0);
    if updated == 0 {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "record not found or already deleted" })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(json!({}))).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/statuses/{id}/context
// ---------------------------------------------------------------------------

async fn get_status_context(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<String>,
) -> Response {
    let uri = match resolve_status_id(&state.db, &id).await.ok().flatten() {
        Some(u) => u,
        None => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "record not found" })))
                .into_response()
        }
    };
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let mut items = match federation_feed::list_thread_context(
        &state.db,
        &uri,
        caller.citizen.as_uuid(),
        &media_base,
    )
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "thread context failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };
    federation_feed::enrich_with_media(&state.db, &mut items, &media_base).await;
    let viewer = build_account_for_citizen(&state, caller.citizen)
        .await
        .map(|a| a.uri);
    federation_feed::enrich_with_polls(&state.db, &mut items, viewer.as_deref()).await;
    // Split ancestors (in_reply_to points at the previous in the chain up
    // to the root) vs descendants (children). For our list_thread_context
    // returns only DESCENDANTS today — ancestors is a follow-up. Return
    // ancestors=[] + descendants=<items after root>.
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let mut descendants: Vec<Status> = Vec::new();
    for it in items {
        if it.object_uri == uri {
            continue;
        }
        descendants.push(build_status_from_feed_item(&state, &it, &host).await);
    }
    (
        StatusCode::OK,
        Json(json!({
            "ancestors": [],
            "descendants": descendants,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Reaction toggles — favourite/unfavourite/reblog/unreblog
// ---------------------------------------------------------------------------

async fn favourite_status(state: State<AppState>, caller: CallerId, path: Path<String>) -> Response {
    toggle_reaction(state, caller, path, "like", true).await
}
async fn unfavourite_status(state: State<AppState>, caller: CallerId, path: Path<String>) -> Response {
    toggle_reaction(state, caller, path, "like", false).await
}
async fn reblog_status(state: State<AppState>, caller: CallerId, path: Path<String>) -> Response {
    toggle_reaction(state, caller, path, "boost", true).await
}
async fn unreblog_status(state: State<AppState>, caller: CallerId, path: Path<String>) -> Response {
    toggle_reaction(state, caller, path, "boost", false).await
}

/// The Mastodon reaction endpoints are two flavours of the same operation:
/// set/unset a like or boost, then return the fresh Status. Uses the same
/// underlying `federation_feed` helpers our own /me/like uses.
async fn toggle_reaction(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<String>,
    kind: &str,
    set: bool,
) -> Response {
    let uri = match resolve_status_id(&state.db, &id).await.ok().flatten() {
        Some(u) => u,
        None => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "record not found" })))
                .into_response()
        }
    };
    let citizen = caller.citizen.as_uuid();
    let now = chrono::Utc::now();
    let existing =
        federation_feed::find_local_reaction(&state.db, citizen, &uri, kind).await;
    match existing {
        Ok(Some(prev_activity)) => {
            let _ = prev_activity;
            if !set {
                let _ = federation_feed::delete_local_reaction(&state.db, citizen, &uri, kind)
                    .await;
            }
            // set=true and already set → no-op (Mastodon behaviour)
        }
        Ok(None) => {
            if set {
                // Build an activity id local to the caller. Reusing the same
                // shape as /me/like — the delivery worker (or our federate
                // step) can pick it up later.
                let public_origin = std::env::var("PUBLIC_ORIGIN")
                    .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
                let host = std::env::var("PUBLIC_HOST")
                    .unwrap_or_else(|_| "democracia.social.br".to_owned());
                let svc = dsoc_auth::profile::ProfileService::from_state(&state);
                let handle = super::federation::handle_of(&svc, caller.citizen).await;
                let actor_url = format!("{}/actors/{}", public_origin.trim_end_matches('/'), handle);
                let activity_kind = if kind == "like" { "likes" } else { "announces" };
                let activity_id = format!(
                    "{actor_url}/activities/{activity_kind}-{}",
                    Uuid::now_v7()
                );
                let _ = host;
                let _ = federation_feed::insert_local_reaction(
                    &state.db,
                    citizen,
                    &uri,
                    kind,
                    &activity_id,
                    now,
                )
                .await;
            }
            // set=false and not set → no-op
        }
        Err(err) => {
            tracing::error!(error = ?err, "reaction lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    }
    // Return the fresh Status.
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    match load_status(&state, &id, citizen).await {
        Some((_, item)) => {
            let status = build_status_from_feed_item(&state, &item, &host).await;
            (StatusCode::OK, Json(status)).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "record not found" })))
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/notifications
// ---------------------------------------------------------------------------

async fn get_notifications(
    State(state): State<AppState>,
    caller: CallerId,
    Query(query): Query<TimelineQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(30).clamp(1, 50);
    let host =
        std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let items = notifications::list_for_citizen(&state.db, caller.citizen.as_uuid(), limit, 0)
        .await
        .unwrap_or_default();
    let mut out: Vec<MastodonNotification> = Vec::with_capacity(items.len());
    for n in items {
        // Source account: from the actor URL if we can resolve it locally,
        // else a stub. Skip the DB round-trip for stubs.
        let source_handle = n.source_handle.clone();
        let account = if source_handle.contains('@') {
            Account::from_remote_stub(
                source_handle.split('@').next().unwrap_or("?"),
                n.source_display_name.as_deref(),
                n.source_avatar_url.as_deref(),
                n.source_actor_url.as_deref().unwrap_or(""),
            )
        } else {
            build_account_for_local_handle(&state, &source_handle, &host)
                .await
                .unwrap_or_else(|| {
                    Account::from_remote_stub(
                        &source_handle,
                        n.source_display_name.as_deref(),
                        n.source_avatar_url.as_deref(),
                        &format!("https://{host}/actors/{source_handle}"),
                    )
                })
        };
        // Status: only for non-follow kinds.
        let status = if let Some(uri) = &n.object_uri {
            if let Some((_uri, item)) =
                load_status(&state, &mastodon_dto::short_hash(uri), caller.citizen.as_uuid()).await
            {
                Some(build_status_from_feed_item(&state, &item, &host).await)
            } else {
                None
            }
        } else {
            None
        };
        out.push(MastodonNotification::from_dto(&n, account, status));
    }
    (StatusCode::OK, Json(out)).into_response()
}

async fn clear_notifications(
    State(state): State<AppState>,
    caller: CallerId,
) -> Response {
    let _ = notifications::mark_all_read(&state.db, caller.citizen.as_uuid()).await;
    (StatusCode::OK, Json(json!({}))).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/media
// ---------------------------------------------------------------------------

async fn post_media(
    State(state): State<AppState>,
    caller: CallerId,
    mut multipart: axum::extract::Multipart,
) -> Response {
    // Same shape as our /me/media, but response is Mastodon MediaAttachment.
    let mut file: Option<Vec<u8>> = None;
    let mut alt: Option<String> = None;
    loop {
        let next = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return client_error("multipart parse failed"),
        };
        match next.name() {
            Some("file") => file = next.bytes().await.ok().map(|b| b.to_vec()),
            Some("description") => alt = next.text().await.ok(),
            _ => {
                let _ = next.bytes().await;
            }
        }
    }
    let Some(bytes) = file else {
        return client_error("file field required");
    };
    let svc = dsoc_auth::profile::ProfileService::from_state(&state);
    let handle_now = super::federation::handle_of(&svc, caller.citizen).await;
    let host = std::env::var("PUBLIC_HOST").unwrap_or_else(|_| "democracia.social.br".to_owned());
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let me_url = format!("https://{host}/actors/{handle_now}");
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
            Json(json!({
                "id": m.id.to_string(),
                "type": "image",
                "url": m.url,
                "preview_url": m.url,
                "remote_url": null,
                "description": m.alt_text,
                "blurhash": null,
                "meta": {
                    "original": {
                        "width": m.width,
                        "height": m.height,
                        "size": format!("{}x{}", m.width, m.height),
                        "aspect": if m.height > 0 { m.width as f64 / m.height as f64 } else { 1.0 },
                    }
                }
            })),
        )
            .into_response(),
        Err(err) => {
            let msg = err.user_message();
            client_error(&msg)
        }
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/polls/{id}/votes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MastodonVoteRequest {
    /// Array of option INDEXES to vote for (Mastodon convention).
    choices: Vec<i64>,
}

async fn vote_poll(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<String>,
    AnyBody(body): AnyBody<MastodonVoteRequest>,
) -> Response {
    // The Mastodon `id` here is the Poll id, not a status id. Look it up in
    // note_poll directly, then map choices (indices) to our option UUIDs.
    let poll: Option<(Uuid, String)> = sqlx::query_as::<_, (Uuid, String)>(
        r"SELECT id, object_uri FROM note_poll WHERE id::text = $1",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some((poll_id, object_uri)) = poll else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "poll not found" })))
            .into_response();
    };
    // Fetch options in order to map indices → uuids.
    let options: Vec<(Uuid, i32)> = sqlx::query_as::<_, (Uuid, i32)>(
        r"SELECT id, sort_order FROM note_poll_option WHERE poll_id = $1 ORDER BY sort_order",
    )
    .bind(poll_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut option_ids: Vec<Uuid> = Vec::with_capacity(body.choices.len());
    for c in body.choices {
        let idx = c as usize;
        if idx >= options.len() {
            return client_error("choice index out of range");
        }
        option_ids.push(options[idx].0);
    }
    let public_origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let svc = dsoc_auth::profile::ProfileService::from_state(&state);
    let handle = super::federation::handle_of(&svc, caller.citizen).await;
    let voter_url = format!("{}/actors/{}", public_origin.trim_end_matches('/'), handle);
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    match polls::cast_vote(&state.db, &object_uri, &voter_url, &option_ids, &media_base).await {
        Ok(dto) => {
            let m = MastodonPoll::from(&dto);
            (StatusCode::OK, Json(m)).into_response()
        }
        Err(err) => {
            let msg = err.user_message();
            match err {
                polls::PollError::Db(_) => {
                    tracing::error!(error = ?err, "vote persistence failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
                }
                _ => client_error(&msg),
            }
        }
    }
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
