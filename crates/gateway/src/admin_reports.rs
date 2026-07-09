//! Admin: fila de denúncias (`note_report`).
//!
//! Fecha o ciclo aberto pela feature de "Denunciar publicação" no menu de
//! 3-pontinhos: cada denúncia entra na tabela e aqui um moderador vê a fila,
//! pode expandir cada uma e marcar como resolvida com notas.
//!
//! - `GET  /admin/reports?status=pending|resolved&limit=&offset=` — lista
//!   com joins pra hidratar autor da nota e denunciante.
//! - `POST /admin/reports/{id}/resolve {notes?}` — marca resolved_at + notas.
//! - `POST /admin/reports/{id}/reopen` — desfaz resolução.
//!
//! Auth: `require_admin` (mesmo padrão dos outros admin_*).

use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/reports", get(list_reports))
        .route("/admin/reports/{id}/resolve", post(resolve_report))
        .route("/admin/reports/{id}/reopen", post(reopen_report))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guard (duplicado dos outros admin_*: pra evitar acoplamento cruzado).
// ---------------------------------------------------------------------------

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    let citizen_id: Uuid = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(unauthorized_resp)?;
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE citizen_id = $1 AND role IN ('owner','admin')
           )",
    )
    .bind(citizen_id)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if !is_admin {
        return Err(forbidden_resp());
    }
    Ok(citizen_id)
}

fn unauthorized_resp() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail("unauthorized", "Autenticação necessária.")),
    )
        .into_response()
}
fn forbidden_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail("forbidden", "Acesso restrito a admins.")),
    )
        .into_response()
}
fn storage_resp(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "admin_reports storage");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    /// pending (default) | resolved | all
    #[serde(default = "default_status")]
    status: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_status() -> String { "pending".to_string() }
fn default_limit() -> i64 { 30 }

#[derive(Debug, Serialize)]
struct ReportDto {
    id: Uuid,
    object_uri: String,
    author_actor_url: String,
    category: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    resolution_notes: Option<String>,
    reporter_handle: Option<String>,
    reporter_display_name: Option<String>,
    /// Quantas denúncias distintas essa mesma nota já acumulou.
    total_for_note: i64,
}

async fn list_reports(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let limit = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let where_clause = match q.status.as_str() {
        "resolved" => "WHERE nr.resolved_at IS NOT NULL",
        "all" => "",
        _ => "WHERE nr.resolved_at IS NULL",
    };
    let sql = format!(
        r"SELECT nr.id,
                 nr.object_uri,
                 nr.author_actor_url,
                 nr.category,
                 nr.reason,
                 nr.created_at,
                 nr.resolved_at,
                 nr.resolution_notes,
                 c.handle,
                 c.display_name,
                 (SELECT count(*) FROM note_report nr2
                   WHERE nr2.object_uri = nr.object_uri) AS total_for_note
            FROM note_report nr
            LEFT JOIN citizen c ON c.id = nr.reporter_id
            {where_clause}
           ORDER BY nr.created_at DESC
           LIMIT $1 OFFSET $2"
    );
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Option<String>,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ),
    >(&sql)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<ReportDto> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        object_uri,
                        author_actor_url,
                        category,
                        reason,
                        created_at,
                        resolved_at,
                        resolution_notes,
                        handle,
                        display_name,
                        total_for_note,
                    )| ReportDto {
                        id,
                        object_uri,
                        author_actor_url,
                        category,
                        reason,
                        created_at,
                        resolved_at,
                        resolution_notes,
                        reporter_handle: handle,
                        reporter_display_name: display_name,
                        total_for_note,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => storage_resp(err),
    }
}

// ---------------------------------------------------------------------------
// Resolve
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ResolveBody {
    #[serde(default)]
    notes: Option<String>,
}

async fn resolve_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<ResolveBody>>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let notes = body.and_then(|Json(b)| b.notes).map(|s| s.trim().to_owned());
    let notes = notes.filter(|s| !s.is_empty());
    let res = sqlx::query(
        r"UPDATE note_report
             SET resolved_at = now(),
                 resolved_by = $2,
                 resolution_notes = $3
           WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(admin_id)
    .bind(notes.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "Denúncia não encontrada ou já resolvida.")),
        )
            .into_response(),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => storage_resp(err),
    }
}

async fn reopen_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let res = sqlx::query(
        r"UPDATE note_report
             SET resolved_at = NULL,
                 resolved_by = NULL,
                 resolution_notes = NULL
           WHERE id = $1 AND resolved_at IS NOT NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => storage_resp(err),
    }
}
