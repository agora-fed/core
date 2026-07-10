//! Anúncios da instância (migration 0510).
//!
//! Endpoints:
//! - `GET  /api/v1/announcements/active` — público. Ativos = published_at NOT NULL,
//!   dentro de starts_at..ends_at (se preenchidos), não dismissed pelo caller.
//! - `POST /api/v1/admin/announcements` — cria (admin).
//! - `GET  /api/v1/admin/announcements` — lista todos, inclusive rascunhos.
//! - `PATCH /api/v1/admin/announcements/{id}` — edita body/severity/janela.
//! - `POST /api/v1/admin/announcements/{id}/publish` — marca published_at=now.
//! - `POST /api/v1/admin/announcements/{id}/unpublish` — zera published_at.
//! - `DELETE /api/v1/admin/announcements/{id}` — apaga.
//! - `POST /api/v1/announcements/{id}/dismiss` — cidadão fecha localmente.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/announcements/active", get(list_active))
        .route("/announcements/{id}/dismiss", post(dismiss))
        .route("/admin/announcements", get(admin_list).post(admin_create))
        .route(
            "/admin/announcements/{id}",
            patch(admin_update).delete(admin_delete),
        )
        .route("/admin/announcements/{id}/publish", post(admin_publish))
        .route("/admin/announcements/{id}/unpublish", post(admin_unpublish))
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
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
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
    tracing::error!(?err, "announcements storage");
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

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AnnouncementDto {
    id: Uuid,
    body: String,
    severity: String,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    published_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    body: String,
    #[serde(default = "default_severity")]
    severity: String,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    #[serde(default)]
    publish_now: bool,
}
fn default_severity() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    body: Option<String>,
    severity: Option<String>,
    starts_at: Option<Option<DateTime<Utc>>>,
    ends_at: Option<Option<DateTime<Utc>>>,
}

// ---------------------------------------------------------------------------
// Public listing
// ---------------------------------------------------------------------------

async fn list_active(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let citizen = caller_citizen(&headers);
    let rows: Result<Vec<AnnouncementDto>, _> = sqlx::query_as::<_, AnnouncementDto>(
        r"SELECT a.id, a.body, a.severity, a.starts_at, a.ends_at, a.published_at, a.created_at
            FROM server_announcement a
           WHERE a.published_at IS NOT NULL
             AND (a.starts_at IS NULL OR a.starts_at <= now())
             AND (a.ends_at   IS NULL OR a.ends_at   >  now())
             AND ($1::uuid IS NULL
                  OR NOT EXISTS (SELECT 1 FROM server_announcement_dismissal d
                                  WHERE d.announcement_id = a.id
                                    AND d.citizen_id = $1))
           ORDER BY a.published_at DESC
           LIMIT 20",
    )
    .bind(citizen)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn dismiss(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized_resp();
    };
    let _ = sqlx::query(
        r"INSERT INTO server_announcement_dismissal (id, announcement_id, citizen_id)
          VALUES ($1, $2, $3)
          ON CONFLICT (announcement_id, citizen_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    ok_json()
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

async fn admin_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<AnnouncementDto>, _> = sqlx::query_as::<_, AnnouncementDto>(
        r"SELECT id, body, severity, starts_at, ends_at, published_at, created_at
            FROM server_announcement
           ORDER BY created_at DESC
           LIMIT 200",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !matches!(body.severity.as_str(), "info" | "warning" | "critical") {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("bad_request", "severity inválida")),
        )
            .into_response();
    }
    let body_txt = body.body.trim();
    if body_txt.is_empty() || body_txt.len() > 4000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "bad_request",
                "texto entre 1 e 4000 chars",
            )),
        )
            .into_response();
    }
    let published_at = if body.publish_now {
        Some(Utc::now())
    } else {
        None
    };
    let id = Uuid::now_v7();
    let res: Result<AnnouncementDto, _> = sqlx::query_as::<_, AnnouncementDto>(
        r"INSERT INTO server_announcement
            (id, body, severity, starts_at, ends_at, published_at, created_by)
          VALUES ($1, $2, $3, $4, $5, $6, $7)
          RETURNING id, body, severity, starts_at, ends_at, published_at, created_at",
    )
    .bind(id)
    .bind(body_txt)
    .bind(&body.severity)
    .bind(body.starts_at)
    .bind(body.ends_at)
    .bind(published_at)
    .bind(admin_id)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(dto) => (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBody>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    // Update dinâmico simplista: um SQL por campo alterado.
    if let Some(t) = body.body.as_deref() {
        let _ = sqlx::query(r"UPDATE server_announcement SET body = $2 WHERE id = $1")
            .bind(id)
            .bind(t.trim())
            .execute(&state.db)
            .await;
    }
    if let Some(s) = body.severity.as_deref() {
        if matches!(s, "info" | "warning" | "critical") {
            let _ = sqlx::query(r"UPDATE server_announcement SET severity = $2 WHERE id = $1")
                .bind(id)
                .bind(s)
                .execute(&state.db)
                .await;
        }
    }
    if let Some(sa) = body.starts_at {
        let _ = sqlx::query(r"UPDATE server_announcement SET starts_at = $2 WHERE id = $1")
            .bind(id)
            .bind(sa)
            .execute(&state.db)
            .await;
    }
    if let Some(ea) = body.ends_at {
        let _ = sqlx::query(r"UPDATE server_announcement SET ends_at = $2 WHERE id = $1")
            .bind(id)
            .bind(ea)
            .execute(&state.db)
            .await;
    }
    ok_json()
}

async fn admin_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(
        r"UPDATE server_announcement SET published_at = now() WHERE id = $1 AND published_at IS NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    ok_json()
}

async fn admin_unpublish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"UPDATE server_announcement SET published_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

async fn admin_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM server_announcement WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}
