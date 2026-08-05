//! Full fediverse: custom emojis + hashtag moderation +
//! auto-delete (migration 0512).
//!
//! Emojis:
//! - `GET  /api/v1/server/emojis` — public, enabled only.
//! - `POST /api/v1/admin/emojis` — multipart {file, shortcode}. PNG/GIF, 128x128 max.
//! - `GET  /api/v1/admin/emojis` — lista completa (inclui disabled).
//! - `PATCH /api/v1/admin/emojis/{id}` — { enabled }.
//! - `DELETE /api/v1/admin/emojis/{id}`.
//!
//! Hashtags:
//! - `GET  /api/v1/admin/hashtags/moderation`.
//! - `POST /api/v1/admin/hashtags/moderation {tag, state, reason?}`.
//! - `DELETE /api/v1/admin/hashtags/moderation/{tag}`.
//!
//! Auto-delete:
//! - `GET  /api/v1/me/preferences/auto_delete` → { days | null }.
//! - `PUT  /api/v1/me/preferences/auto_delete` → { days }.
//!
//! (the deletion worker is called in worker.rs; in this slice we only
//! persist the preference.)

use axum::extract::{Json, Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::io::Cursor;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/server/emojis", get(public_emojis))
        .route(
            "/admin/emojis",
            get(admin_list_emojis).post(admin_upload_emoji),
        )
        .route(
            "/admin/emojis/{id}",
            patch(admin_toggle_emoji).delete(admin_delete_emoji),
        )
        .route(
            "/admin/hashtags/moderation",
            get(admin_list_hashtags).post(admin_upsert_hashtag),
        )
        .route(
            "/admin/hashtags/moderation/{tag}",
            delete(admin_delete_hashtag),
        )
        .route(
            "/me/preferences/auto_delete",
            get(get_auto_delete).put(put_auto_delete),
        )
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    let citizen = caller_citizen(headers).ok_or_else(unauthorized_resp)?;
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (SELECT 1 FROM admin_role_binding
                        WHERE citizen_id = $1 AND role IN ('owner','admin'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if !is_admin {
        return Err(forbidden_resp());
    }
    Ok(citizen)
}

fn unauthorized_resp() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}
fn forbidden_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "forbidden",
            "Acesso restrito a admins.",
        )),
    )
        .into_response()
}
fn storage_err(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "fediverso_admin storage");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}
fn ok_json() -> Response {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}
fn bad(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("bad_request", msg)),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Emojis
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct EmojiDto {
    id: Uuid,
    shortcode: String,
    url: String,
    enabled: bool,
    created_at: DateTime<Utc>,
}

async fn public_emojis(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<EmojiDto>, _> = sqlx::query_as::<_, EmojiDto>(
        r"SELECT id, shortcode, url, enabled, created_at
            FROM custom_emoji
           WHERE enabled = true
           ORDER BY shortcode",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_list_emojis(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<EmojiDto>, _> = sqlx::query_as::<_, EmojiDto>(
        r"SELECT id, shortcode, url, enabled, created_at
            FROM custom_emoji
           ORDER BY shortcode",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

const EMOJI_SIZE: u32 = 128;
const MAX_EMOJI_BYTES: usize = 512 * 1024; // 512 KB antes de resize.

async fn admin_upload_emoji(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let mut file: Option<Vec<u8>> = None;
    let mut shortcode: Option<String> = None;
    loop {
        let next = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return bad("upload inválido"),
        };
        match next.name() {
            Some("file") => match next.bytes().await {
                Ok(b) => file = Some(b.to_vec()),
                Err(_) => return bad("falha ao ler o arquivo"),
            },
            Some("shortcode") => shortcode = next.text().await.ok(),
            _ => {
                let _ = next.bytes().await;
            }
        }
    }
    let Some(raw) = file else {
        return bad("envie o arquivo no campo `file`");
    };
    let Some(sc) = shortcode
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    else {
        return bad("envie shortcode");
    };
    if !sc
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || sc.len() < 2
        || sc.len() > 32
    {
        return bad("shortcode inválido (2-32 chars: A-Z a-z 0-9 _ -)");
    }
    if raw.len() > MAX_EMOJI_BYTES {
        return bad("arquivo muito grande (máx 512 KB)");
    }
    // Decode + resize to a square EMOJI_SIZE (keeping the aspect ratio). Encode PNG.
    let bytes = match tokio::task::spawn_blocking(move || process_emoji(&raw)).await {
        Ok(Ok(b)) => b,
        Ok(Err(msg)) => return bad(msg),
        Err(_) => return storage_err("blocking runtime"),
    };
    let Some(storage) = state.storage.as_ref() else {
        return bad("storage não configurado");
    };
    let id = Uuid::now_v7();
    let key = format!("emoji/{}.png", id.simple());
    if let Err(e) = storage.put(&key, "image/png", bytes).await {
        return storage_err(e);
    }
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let url = format!("{media_base}/{key}");
    let res = sqlx::query(
        r"INSERT INTO custom_emoji (id, shortcode, url, enabled, created_by)
          VALUES ($1, $2, $3, true, $4)",
    )
    .bind(id)
    .bind(&sc)
    .bind(&url)
    .bind(admin)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "id": id,
                "shortcode": sc,
                "url": url,
                "enabled": true,
            }))),
        )
            .into_response(),
        Err(err) if err.to_string().contains("unique") => bad("shortcode já existe"),
        Err(err) => storage_err(err),
    }
}

fn process_emoji(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    let format = image::guess_format(raw).map_err(|_| "não é imagem")?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) {
        return Err("formato não suportado (PNG, JPG, WebP, GIF)");
    }
    let img = image::load_from_memory_with_format(raw, format).map_err(|_| "imagem inválida")?;
    let shrunk = img.resize(
        EMOJI_SIZE,
        EMOJI_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    shrunk
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(|_| "encode png falhou")?;
    Ok(buf)
}

#[derive(Debug, Deserialize)]
struct ToggleEmoji {
    enabled: bool,
}
async fn admin_toggle_emoji(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ToggleEmoji>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"UPDATE custom_emoji SET enabled = $2 WHERE id = $1")
        .bind(id)
        .bind(body.enabled)
        .execute(&state.db)
        .await;
    ok_json()
}

async fn admin_delete_emoji(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM custom_emoji WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

// ---------------------------------------------------------------------------
// Hashtags
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct HashtagModDto {
    tag: String,
    state: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
}

async fn admin_list_hashtags(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<HashtagModDto>, _> = sqlx::query_as::<_, HashtagModDto>(
        r"SELECT tag, state, reason, created_at
            FROM hashtag_moderation
           ORDER BY state, tag",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct HashtagUpsert {
    tag: String,
    state: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn admin_upsert_hashtag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<HashtagUpsert>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    if !matches!(body.state.as_str(), "banned" | "promoted") {
        return bad("state deve ser banned ou promoted");
    }
    let tag = body.tag.trim().trim_start_matches('#').to_ascii_lowercase();
    if tag.is_empty() || tag.len() > 60 {
        return bad("tag inválida");
    }
    let reason = body.reason.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res = sqlx::query(
        r"INSERT INTO hashtag_moderation (tag, state, reason)
          VALUES ($1, $2, $3)
          ON CONFLICT (tag) DO UPDATE SET state = EXCLUDED.state, reason = EXCLUDED.reason",
    )
    .bind(&tag)
    .bind(&body.state)
    .bind(reason.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(),
        Err(err) => storage_err(err),
    }
}

async fn admin_delete_hashtag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(tag): Path<String>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let tag = tag.trim_start_matches('#').to_ascii_lowercase();
    let _ = sqlx::query(r"DELETE FROM hashtag_moderation WHERE tag = $1")
        .bind(&tag)
        .execute(&state.db)
        .await;
    ok_json()
}

// ---------------------------------------------------------------------------
// Auto-delete
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct AutoDeleteDto {
    days: Option<i32>,
}

async fn get_auto_delete(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized_resp();
    };
    let days: Option<i32> =
        sqlx::query_scalar(r"SELECT auto_delete_notes_older_than_days FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
            .flatten();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(AutoDeleteDto { days })),
    )
        .into_response()
}

async fn put_auto_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AutoDeleteDto>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized_resp();
    };
    if let Some(d) = body.days {
        if !(7..=3650).contains(&d) {
            return bad("days entre 7 e 3650");
        }
    }
    let _ = sqlx::query(r"UPDATE citizen SET auto_delete_notes_older_than_days = $2 WHERE id = $1")
        .bind(citizen)
        .bind(body.days)
        .execute(&state.db)
        .await;
    ok_json()
}
