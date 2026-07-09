//! # Social-graph endpoints (0.20.0-beta fase 2A — migration 0500).
//!
//! Every Mastodon-parity endpoint that reads or mutates one of the six tables
//! introduced by migration `0500_bookmarks_mutes_blocks_filters_lists.sql`:
//!
//! * Bookmarks: `GET /bookmarks`, `POST /statuses/{id}/bookmark`,
//!   `POST /statuses/{id}/unbookmark`.
//! * Mutes: `GET /mutes`, `POST /accounts/{citizen_id}/mute|unmute`.
//! * Blocks: `GET /blocks`, `POST /accounts/{citizen_id}/block|unblock`.
//! * Content filters: `GET /filters`, `POST /filters`, `DELETE /filters/{id}`.
//! * Lists: `GET|POST /lists`, `PUT|DELETE /lists/{id}`,
//!   `GET|POST|DELETE /lists/{id}/accounts`,
//!   `GET /timelines/list/{id}`.
//!
//! Every handler is guarded by cookie/bearer auth via the `x-dsoc-citizen-id`
//! header — same pattern as `admin_ext.rs` and `me_settings.rs`. Every SQL
//! statement is a RUNTIME `sqlx::query*` call (never the compile-checked
//! macros) so the committed `.sqlx/` cache does not need regenerating.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use dsoc_app::AppState;
use dsoc_api_contract::ApiResponse;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        // Bookmarks
        .route("/bookmarks", get(list_bookmarks))
        .route("/statuses/{id}/bookmark", post(bookmark_status))
        .route("/statuses/{id}/unbookmark", post(unbookmark_status))
        // Bookmark de URIs cruas (notas remotas do outbox proxy, sem UUID local).
        .route("/me/bookmarks", post(bookmark_uri).delete(unbookmark_uri))
        .route("/me/bookmarks/status", get(bookmark_status_of))
        // Mutes
        .route("/mutes", get(list_mutes))
        .route("/accounts/{citizen_id}/mute", post(mute_account))
        .route("/accounts/{citizen_id}/unmute", post(unmute_account))
        // Blocks
        .route("/blocks", get(list_blocks))
        .route("/accounts/{citizen_id}/block", post(block_account))
        .route("/accounts/{citizen_id}/unblock", post(unblock_account))
        // Content filters
        .route("/filters", get(list_filters).post(create_filter))
        .route("/filters/{id}", delete(delete_filter))
        // Lists
        .route("/lists", get(list_lists).post(create_list))
        .route(
            "/lists/{id}",
            put(update_list).delete(delete_list),
        )
        .route(
            "/lists/{id}/accounts",
            get(list_list_accounts)
                .post(add_list_accounts)
                .delete(remove_list_accounts),
        )
        .route("/timelines/list/{id}", get(list_timeline))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guards / helpers (mirror admin_ext.rs + me_settings.rs)
// ---------------------------------------------------------------------------

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail("http_401", "Autenticação necessária.")),
    )
        .into_response()
}

fn not_found(msg: &'static str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()>::fail("http_404", msg)),
    )
        .into_response()
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

fn bad_request(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("http_400", msg)),
    )
        .into_response()
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// The public origin this instance federates under, trailing-slash-free.
/// Mirrors `federation::public_origin`.
fn public_origin() -> String {
    std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
        .trim_end_matches('/')
        .to_owned()
}

/// Resolve a local citizen id to its `<public_origin>/actors/<handle>` URL.
/// Returns `Ok(None)` when the citizen has no handle (e.g. anonymous account
/// or one that has never claimed a handle).
async fn actor_url_for_citizen(
    db: &PgPool,
    citizen_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as(r"SELECT handle FROM citizen WHERE id = $1")
            .bind(citizen_id)
            .fetch_optional(db)
            .await?;
    let handle = match row {
        Some((Some(h),)) => h,
        _ => return Ok(None),
    };
    Ok(Some(format!("{}/actors/{}", public_origin(), handle)))
}

// ---------------------------------------------------------------------------
// Common pagination
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

impl PageQuery {
    fn limit(&self, default_: i64, max: i64) -> i64 {
        self.limit.unwrap_or(default_).clamp(1, max)
    }
    fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct BookmarkDto {
    object_uri: String,
    created_at: DateTime<Utc>,
}

async fn list_bookmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let limit = page.limit(20, 100);
    let offset = page.offset();
    let rows: Result<Vec<(String, DateTime<Utc>)>, _> = sqlx::query_as(
        r"SELECT object_uri, created_at
            FROM note_bookmark
           WHERE citizen_id = $1
           ORDER BY created_at DESC
           LIMIT $2 OFFSET $3",
    )
    .bind(citizen)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<BookmarkDto> = rows
                .into_iter()
                .map(|(object_uri, created_at)| BookmarkDto { object_uri, created_at })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_bookmarks");
            server_error()
        }
    }
}

async fn bookmark_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    // Aceita tanto o UUID de mastodon_status_id quanto o object_uri cru
    // (AP URL, e.g. https://mastodon.social/users/x/statuses/123) — o
    // segundo caso cobre notas do outbox remoto que ainda não têm
    // mapeamento local, tipicamente vindas do proxy do perfil.
    let object_uri = if id.starts_with("https://") || id.starts_with("http://") {
        id.clone()
    } else {
        match crate::mastodon_api::resolve_status_id(&state.db, &id).await {
            Ok(Some(uri)) => uri,
            Ok(None) => return not_found("Nota não encontrada."),
            Err(err) => {
                tracing::error!(?err, "bookmark resolve_status_id");
                return server_error();
            }
        }
    };
    let res = sqlx::query(
        r"INSERT INTO note_bookmark (id, citizen_id, object_uri)
          VALUES ($1, $2, $3)
          ON CONFLICT (citizen_id, object_uri) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(citizen)
    .bind(&object_uri)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "bookmark insert");
            server_error()
        }
    }
}

async fn unbookmark_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let object_uri = if id.starts_with("https://") || id.starts_with("http://") {
        id.clone()
    } else {
        match crate::mastodon_api::resolve_status_id(&state.db, &id).await {
            Ok(Some(uri)) => uri,
            // Unbookmark of an unknown id is idempotent — the row cannot exist.
            Ok(None) => {
                return (
                    StatusCode::OK,
                    Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
                )
                    .into_response();
            }
            Err(err) => {
                tracing::error!(?err, "unbookmark resolve_status_id");
                return server_error();
            }
        }
    };
    let res = sqlx::query(
        r"DELETE FROM note_bookmark WHERE citizen_id = $1 AND object_uri = $2",
    )
    .bind(citizen)
    .bind(&object_uri)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "unbookmark delete");
            server_error()
        }
    }
}

/// Body for POST/DELETE `/api/v1/me/bookmarks` — apenas `object_uri` cru.
#[derive(Debug, Deserialize)]
struct BookmarkUriBody {
    object_uri: String,
}

async fn bookmark_uri(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BookmarkUriBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let uri = body.object_uri.trim();
    if uri.is_empty() || uri.len() > 2048 {
        return not_found("object_uri inválido");
    }
    let res = sqlx::query(
        r"INSERT INTO note_bookmark (id, citizen_id, object_uri)
          VALUES ($1, $2, $3)
          ON CONFLICT (citizen_id, object_uri) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(citizen)
    .bind(uri)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "bookmark_uri insert");
            server_error()
        }
    }
}

async fn unbookmark_uri(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BookmarkUriBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"DELETE FROM note_bookmark WHERE citizen_id = $1 AND object_uri = $2",
    )
    .bind(citizen)
    .bind(body.object_uri.trim())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "unbookmark_uri delete");
            server_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct BookmarkStatusQuery {
    object_uri: String,
}

async fn bookmark_status_of(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<BookmarkStatusQuery>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let row: Result<Option<(i64,)>, _> = sqlx::query_as::<_, (i64,)>(
        r"SELECT count(*)
            FROM note_bookmark
           WHERE citizen_id = $1 AND object_uri = $2",
    )
    .bind(citizen)
    .bind(q.object_uri.trim())
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some((n,))) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "bookmarked": n > 0 }))),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "bookmarked": false }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "bookmark_status_of");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Mutes
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct MuteDto {
    target_actor_url: String,
    hide_notifications: bool,
    created_at: DateTime<Utc>,
}

async fn list_mutes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let limit = page.limit(40, 200);
    let offset = page.offset();
    let rows: Result<Vec<(String, bool, DateTime<Utc>)>, _> = sqlx::query_as(
        r"SELECT target_actor_url, hide_notifications, created_at
            FROM actor_mute
           WHERE citizen_id = $1
           ORDER BY created_at DESC
           LIMIT $2 OFFSET $3",
    )
    .bind(citizen)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<MuteDto> = rows
                .into_iter()
                .map(|(target_actor_url, hide_notifications, created_at)| MuteDto {
                    target_actor_url,
                    hide_notifications,
                    created_at,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_mutes");
            server_error()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct MuteBody {
    #[serde(default = "default_true")]
    notifications: bool,
}
fn default_true() -> bool { true }

async fn mute_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    body: Option<Json<MuteBody>>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let hide_notifications = body.map_or(true, |Json(b)| b.notifications);
    let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
        Ok(Some(url)) => url,
        Ok(None) => return not_found("Conta alvo não encontrada."),
        Err(err) => {
            tracing::error!(?err, "mute lookup handle");
            return server_error();
        }
    };
    let res = sqlx::query(
        r"INSERT INTO actor_mute (id, citizen_id, target_actor_url, hide_notifications)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (citizen_id, target_actor_url)
          DO UPDATE SET hide_notifications = EXCLUDED.hide_notifications",
    )
    .bind(Uuid::now_v7())
    .bind(citizen)
    .bind(&target_actor_url)
    .bind(hide_notifications)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "mute insert");
            server_error()
        }
    }
}

async fn unmute_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
        Ok(Some(url)) => url,
        Ok(None) => {
            // Idempotent: nothing to unmute.
            return (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!(?err, "unmute lookup handle");
            return server_error();
        }
    };
    let res = sqlx::query(
        r"DELETE FROM actor_mute WHERE citizen_id = $1 AND target_actor_url = $2",
    )
    .bind(citizen)
    .bind(&target_actor_url)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "unmute delete");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct BlockDto {
    target_actor_url: String,
    created_at: DateTime<Utc>,
}

async fn list_blocks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let limit = page.limit(40, 200);
    let offset = page.offset();
    let rows: Result<Vec<(String, DateTime<Utc>)>, _> = sqlx::query_as(
        r"SELECT target_actor_url, created_at
            FROM actor_block
           WHERE citizen_id = $1
           ORDER BY created_at DESC
           LIMIT $2 OFFSET $3",
    )
    .bind(citizen)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<BlockDto> = rows
                .into_iter()
                .map(|(target_actor_url, created_at)| BlockDto {
                    target_actor_url,
                    created_at,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_blocks");
            server_error()
        }
    }
}

async fn block_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
        Ok(Some(url)) => url,
        Ok(None) => return not_found("Conta alvo não encontrada."),
        Err(err) => {
            tracing::error!(?err, "block lookup handle");
            return server_error();
        }
    };
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "block tx begin");
            return server_error();
        }
    };
    if let Err(err) = sqlx::query(
        r"INSERT INTO actor_block (id, citizen_id, target_actor_url)
          VALUES ($1, $2, $3)
          ON CONFLICT (citizen_id, target_actor_url) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(citizen)
    .bind(&target_actor_url)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "block insert");
        return server_error();
    }
    // Also drop any follow relation (both directions) between caller and
    // target. Follow rows key by citizen_id + remote_actor_url; the caller's
    // own actor_url is what a REMOTE follower would store, so we cover both
    // sides by matching on caller.citizen_id AND on target's actor_url.
    let caller_actor_url = match actor_url_for_citizen(&state.db, citizen).await {
        Ok(url) => url,
        Err(err) => {
            tracing::error!(?err, "block: caller actor_url lookup");
            return server_error();
        }
    };
    if let Err(err) = sqlx::query(
        r"DELETE FROM federation_follow
           WHERE (citizen_id = $1 AND remote_actor_url = $2)
              OR (citizen_id = $3
                  AND $4::text IS NOT NULL
                  AND remote_actor_url = $4)",
    )
    .bind(citizen)
    .bind(&target_actor_url)
    .bind(target_id)
    .bind(caller_actor_url.as_deref())
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "block: drop follows");
        return server_error();
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "block tx commit");
        return server_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}

async fn unblock_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
        Ok(Some(url)) => url,
        Ok(None) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!(?err, "unblock lookup handle");
            return server_error();
        }
    };
    let res = sqlx::query(
        r"DELETE FROM actor_block WHERE citizen_id = $1 AND target_actor_url = $2",
    )
    .bind(citizen)
    .bind(&target_actor_url)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "unblock delete");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Content filters
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct FilterDto {
    id: Uuid,
    phrase: String,
    context: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

async fn list_filters(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let rows: Result<
        Vec<(Uuid, String, Vec<String>, Option<DateTime<Utc>>, DateTime<Utc>)>,
        _,
    > = sqlx::query_as(
        r"SELECT id, phrase, context, expires_at, created_at
            FROM content_filter
           WHERE citizen_id = $1
           ORDER BY created_at DESC",
    )
    .bind(citizen)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<FilterDto> = rows
                .into_iter()
                .map(|(id, phrase, context, expires_at, created_at)| FilterDto {
                    id,
                    phrase,
                    context,
                    expires_at,
                    created_at,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_filters");
            server_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateFilterBody {
    phrase: String,
    #[serde(default)]
    context: Vec<String>,
    expires_in: Option<i64>,
}

async fn create_filter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateFilterBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let phrase = body.phrase.trim().to_owned();
    if phrase.is_empty() || phrase.len() > 400 {
        return bad_request("A frase do filtro deve ter entre 1 e 400 caracteres.");
    }
    let context = if body.context.is_empty() {
        vec!["home".to_owned()]
    } else {
        body.context.clone()
    };
    let expires_at: Option<DateTime<Utc>> = body
        .expires_in
        .filter(|s| *s > 0)
        .and_then(|secs| Duration::try_seconds(secs).map(|d| Utc::now() + d));
    let id = Uuid::now_v7();
    let res: Result<(Uuid, String, Vec<String>, Option<DateTime<Utc>>, DateTime<Utc>), _> =
        sqlx::query_as(
            r"INSERT INTO content_filter (id, citizen_id, phrase, context, expires_at)
              VALUES ($1, $2, $3, $4, $5)
              RETURNING id, phrase, context, expires_at, created_at",
        )
        .bind(id)
        .bind(citizen)
        .bind(&phrase)
        .bind(&context)
        .bind(expires_at)
        .fetch_one(&state.db)
        .await;
    match res {
        Ok((id, phrase, context, expires_at, created_at)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(FilterDto {
                id,
                phrase,
                context,
                expires_at,
                created_at,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "create_filter");
            server_error()
        }
    }
}

async fn delete_filter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(filter_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"DELETE FROM content_filter WHERE id = $1 AND citizen_id = $2",
    )
    .bind(filter_id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => not_found("Filtro não encontrado."),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "delete_filter");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ListDto {
    id: Uuid,
    title: String,
    replies_policy: String,
    member_count: i64,
}

#[derive(Debug, Serialize)]
struct ListRowDto {
    id: Uuid,
    title: String,
    replies_policy: String,
}

async fn list_lists(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let rows: Result<Vec<(Uuid, String, String, i64)>, _> = sqlx::query_as(
        r"SELECT l.id, l.title, l.replies_policy,
                 (SELECT count(*) FROM actor_list_member m WHERE m.list_id = l.id) AS member_count
            FROM actor_list l
           WHERE l.citizen_id = $1
           ORDER BY l.created_at ASC",
    )
    .bind(citizen)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<ListDto> = rows
                .into_iter()
                .map(|(id, title, replies_policy, member_count)| ListDto {
                    id,
                    title,
                    replies_policy,
                    member_count,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_lists");
            server_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListBody {
    title: String,
    replies_policy: Option<String>,
}

fn validate_replies_policy(rp: &str) -> bool {
    matches!(rp, "followed" | "list" | "none")
}

async fn create_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ListBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let title = body.title.trim().to_owned();
    if title.is_empty() || title.len() > 100 {
        return bad_request("O título da lista deve ter entre 1 e 100 caracteres.");
    }
    let rp = body.replies_policy.unwrap_or_else(|| "list".to_owned());
    if !validate_replies_policy(&rp) {
        return bad_request("replies_policy inválido.");
    }
    let id = Uuid::now_v7();
    let res: Result<(Uuid, String, String), _> = sqlx::query_as(
        r"INSERT INTO actor_list (id, citizen_id, title, replies_policy)
          VALUES ($1, $2, $3, $4)
          RETURNING id, title, replies_policy",
    )
    .bind(id)
    .bind(citizen)
    .bind(&title)
    .bind(&rp)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok((id, title, replies_policy)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ListRowDto { id, title, replies_policy })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "create_list");
            server_error()
        }
    }
}

async fn update_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
    Json(body): Json<ListBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let title = body.title.trim().to_owned();
    if title.is_empty() || title.len() > 100 {
        return bad_request("O título da lista deve ter entre 1 e 100 caracteres.");
    }
    let rp = body.replies_policy.unwrap_or_else(|| "list".to_owned());
    if !validate_replies_policy(&rp) {
        return bad_request("replies_policy inválido.");
    }
    let res: Result<Option<(Uuid, String, String)>, _> = sqlx::query_as(
        r"UPDATE actor_list
             SET title = $3, replies_policy = $4
           WHERE id = $1 AND citizen_id = $2
           RETURNING id, title, replies_policy",
    )
    .bind(list_id)
    .bind(citizen)
    .bind(&title)
    .bind(&rp)
    .fetch_optional(&state.db)
    .await;
    match res {
        Ok(Some((id, title, replies_policy))) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ListRowDto { id, title, replies_policy })),
        )
            .into_response(),
        Ok(None) => not_found("Lista não encontrada."),
        Err(err) => {
            tracing::error!(?err, "update_list");
            server_error()
        }
    }
}

async fn delete_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"DELETE FROM actor_list WHERE id = $1 AND citizen_id = $2",
    )
    .bind(list_id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => not_found("Lista não encontrada."),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "delete_list");
            server_error()
        }
    }
}

/// Verify the list belongs to the caller. Returns `Ok(true)` when it does,
/// `Ok(false)` when it doesn't (caller should return 404), or an error.
async fn owns_list(
    db: &PgPool,
    list_id: Uuid,
    citizen: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r"SELECT 1 FROM actor_list WHERE id = $1 AND citizen_id = $2 LIMIT 1",
    )
    .bind(list_id)
    .bind(citizen)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

#[derive(Debug, Serialize)]
struct ListMemberDto {
    target_actor_url: String,
    created_at: DateTime<Utc>,
}

async fn list_list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match owns_list(&state.db, list_id, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_found("Lista não encontrada."),
        Err(err) => {
            tracing::error!(?err, "list_list_accounts: owns_list");
            return server_error();
        }
    }
    let rows: Result<Vec<(String, DateTime<Utc>)>, _> = sqlx::query_as(
        r"SELECT target_actor_url, created_at
            FROM actor_list_member
           WHERE list_id = $1
           ORDER BY created_at DESC",
    )
    .bind(list_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<ListMemberDto> = rows
                .into_iter()
                .map(|(target_actor_url, created_at)| ListMemberDto {
                    target_actor_url,
                    created_at,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_list_accounts");
            server_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListAccountsBody {
    account_ids: Vec<Uuid>,
}

async fn add_list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
    Json(body): Json<ListAccountsBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match owns_list(&state.db, list_id, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_found("Lista não encontrada."),
        Err(err) => {
            tracing::error!(?err, "add_list_accounts: owns_list");
            return server_error();
        }
    }
    for target_id in body.account_ids {
        let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
            Ok(Some(url)) => url,
            // Skip unknown / handle-less citizens silently — the endpoint is
            // batch and idempotent.
            Ok(None) => continue,
            Err(err) => {
                tracing::error!(?err, "add_list_accounts: lookup handle");
                return server_error();
            }
        };
        if let Err(err) = sqlx::query(
            r"INSERT INTO actor_list_member (list_id, target_actor_url)
              VALUES ($1, $2)
              ON CONFLICT (list_id, target_actor_url) DO NOTHING",
        )
        .bind(list_id)
        .bind(&target_actor_url)
        .execute(&state.db)
        .await
        {
            tracing::error!(?err, "add_list_accounts: insert");
            return server_error();
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}

async fn remove_list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
    Json(body): Json<ListAccountsBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match owns_list(&state.db, list_id, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_found("Lista não encontrada."),
        Err(err) => {
            tracing::error!(?err, "remove_list_accounts: owns_list");
            return server_error();
        }
    }
    for target_id in body.account_ids {
        let target_actor_url = match actor_url_for_citizen(&state.db, target_id).await {
            Ok(Some(url)) => url,
            Ok(None) => continue,
            Err(err) => {
                tracing::error!(?err, "remove_list_accounts: lookup handle");
                return server_error();
            }
        };
        if let Err(err) = sqlx::query(
            r"DELETE FROM actor_list_member
               WHERE list_id = $1 AND target_actor_url = $2",
        )
        .bind(list_id)
        .bind(&target_actor_url)
        .execute(&state.db)
        .await
        {
            tracing::error!(?err, "remove_list_accounts: delete");
            return server_error();
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct ListTimelineItemDto {
    object_uri: String,
    actor_url: String,
    content: String,
    published_at: DateTime<Utc>,
    actor_display_name: Option<String>,
}

async fn list_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(list_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match owns_list(&state.db, list_id, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_found("Lista não encontrada."),
        Err(err) => {
            tracing::error!(?err, "list_timeline: owns_list");
            return server_error();
        }
    }
    let limit = page.limit(20, 50);
    let offset = page.offset();
    let rows: Result<
        Vec<(String, String, String, DateTime<Utc>, Option<String>)>,
        _,
    > = sqlx::query_as(
        r"SELECT t.object_uri, t.actor_url, t.content_html, t.published_at, t.actor_display_name
            FROM federation_timeline_entry t
           WHERE t.actor_url IN (
                 SELECT target_actor_url FROM actor_list_member WHERE list_id = $1
             )
             AND t.deleted_at IS NULL
           ORDER BY t.published_at DESC
           LIMIT $2 OFFSET $3",
    )
    .bind(list_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<ListTimelineItemDto> = rows
                .into_iter()
                .map(|(object_uri, actor_url, content, published_at, actor_display_name)| {
                    ListTimelineItemDto {
                        object_uri,
                        actor_url,
                        content,
                        published_at,
                        actor_display_name,
                    }
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_timeline");
            server_error()
        }
    }
}
