//! # Config dos fóruns (F3) — painel admin.
//!
//! Onde o admin da plataforma cura o **e-mail institucional** de cada fórum, os
//! **patamares** de envio e os **moderadores**. Runtime queries (padrão
//! politico_contacts). O carteiro que consome os `forum_dispatch` pendentes vive
//! em [`crate::forum_mailer`].
//!
//! - `GET    /admin/forums?q=&limit=&offset=`      — lista com e-mail + pendências.
//! - `PATCH  /admin/forums/{id}`                   — contact_email / thresholds.
//! - `GET    /admin/forums/{id}/moderators`        — lista moderadores (handle).
//! - `POST   /admin/forums/{id}/moderators`        — adiciona por handle.
//! - `DELETE /admin/forums/{id}/moderators/{cid}`  — remove.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/forums", get(list))
        .route("/admin/forums/{id}", patch(update))
        .route(
            "/admin/forums/{id}/moderators",
            get(mods_list).post(mods_add),
        )
        .route("/admin/forums/{id}/moderators/{cid}", delete(mods_remove))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}
fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<(), Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        ));
    };
    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admin_role_binding WHERE citizen_id=$1 AND role IN ('owner','admin'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if is_admin {
        Ok(())
    } else {
        Err(fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Requer administrador.",
        ))
    }
}

#[derive(Debug, Deserialize)]
struct ListParams {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AdminForumRow {
    id: Uuid,
    full_path: String,
    name: String,
    kind: String,
    esfera: Option<String>,
    contact_email: Option<String>,
    avatar_url: Option<String>,
    banner_url: Option<String>,
    thresholds: Vec<i32>,
    moderator_count: i64,
    pending_dispatches: i64,
    topic_count: i64,
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let q = p.q.unwrap_or_default();
    let like = format!("%{}%", q.trim().to_lowercase());
    let rows: Result<Vec<AdminForumRow>, _> = sqlx::query_as(
        r"SELECT f.id, f.full_path, f.name, f.kind, f.esfera, f.contact_email,
                 f.avatar_url, f.banner_url, f.thresholds,
                 (SELECT count(*) FROM forum_moderator m WHERE m.forum_id = f.id) AS moderator_count,
                 (SELECT count(*) FROM forum_dispatch d JOIN forum_topic t ON t.id = d.topic_id
                   WHERE t.forum_id = f.id AND d.sent_at IS NULL) AS pending_dispatches,
                 (SELECT count(*) FROM forum_topic t WHERE t.forum_id = f.id) AS topic_count
            FROM forum f
           WHERE f.hidden_at IS NULL
             AND ($1 = '%%' OR lower(f.full_path) LIKE $1 OR lower(f.name) LIKE $1)
           ORDER BY f.full_path
           LIMIT $2 OFFSET $3",
    )
    .bind(&like)
    .bind(p.limit.unwrap_or(50).clamp(1, 200))
    .bind(p.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: list falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateForumRequest {
    /// `Some("a@b")` define; `Some("")` limpa; `None` mantém.
    contact_email: Option<String>,
    /// Patamares novos (crescentes, positivos, máx. 10). `None` mantém.
    thresholds: Option<Vec<i32>>,
    /// Logo — `Some("")` limpa; `None` mantém.
    avatar_url: Option<String>,
    /// Capa — `Some("")` limpa; `None` mantém.
    banner_url: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateForumRequest>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    if let Some(ts) = &req.thresholds {
        let ok = !ts.is_empty()
            && ts.len() <= 10
            && ts.iter().all(|t| *t > 0)
            && ts.windows(2).all(|w| w[0] < w[1]);
        if !ok {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_input",
                "Patamares devem ser positivos, crescentes e no máximo 10.",
            );
        }
    }
    if let Some(email) = &req.contact_email {
        let e = email.trim();
        if !e.is_empty() && (!e.contains('@') || e.contains(' ')) {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_input",
                "E-mail institucional inválido.",
            );
        }
    }
    let email_update = req.contact_email.is_some();
    let email_value: Option<String> = req.contact_email.and_then(|e| {
        let e = e.trim().to_owned();
        if e.is_empty() {
            None
        } else {
            Some(e)
        }
    });
    let norm = |v: Option<String>| -> (bool, Option<String>) {
        match v {
            None => (false, None),
            Some(s) => {
                let s = s.trim().to_owned();
                (true, if s.is_empty() { None } else { Some(s) })
            }
        }
    };
    let (av_up, av_val) = norm(req.avatar_url);
    let (bn_up, bn_val) = norm(req.banner_url);
    let res = sqlx::query(
        r"UPDATE forum SET
             contact_email = CASE WHEN $2 THEN $3 ELSE contact_email END,
             thresholds    = COALESCE($4, thresholds),
             avatar_url    = CASE WHEN $5 THEN $6 ELSE avatar_url END,
             banner_url    = CASE WHEN $7 THEN $8 ELSE banner_url END
           WHERE id = $1",
    )
    .bind(id)
    .bind(email_update)
    .bind(email_value)
    .bind(req.thresholds)
    .bind(av_up)
    .bind(av_val)
    .bind(bn_up)
    .bind(bn_val)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Ok(_) => fail(StatusCode::NOT_FOUND, "not_found", "Fórum não encontrado."),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: update falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ModeratorRow {
    citizen_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
}

async fn mods_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let rows: Result<Vec<ModeratorRow>, _> = sqlx::query_as(
        r"SELECT m.citizen_id, c.handle, c.display_name
            FROM forum_moderator m JOIN citizen c ON c.id = m.citizen_id
           WHERE m.forum_id = $1 ORDER BY m.created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_list falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AddModeratorRequest {
    handle: String,
}

async fn mods_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<AddModeratorRequest>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let handle = req.handle.trim().trim_start_matches('@').to_lowercase();
    let citizen: Option<Uuid> = sqlx::query_scalar("SELECT id FROM citizen WHERE handle = $1")
        .bind(&handle)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let Some(citizen) = citizen else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Nenhum cidadão com esse @handle.",
        );
    };
    let res = sqlx::query(
        "INSERT INTO forum_moderator (forum_id, citizen_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"citizen_id": citizen}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_add falhou");
            storage_error()
        }
    }
}

async fn mods_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, citizen_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let res = sqlx::query("DELETE FROM forum_moderator WHERE forum_id = $1 AND citizen_id = $2")
        .bind(id)
        .bind(citizen_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_remove falhou");
            storage_error()
        }
    }
}
