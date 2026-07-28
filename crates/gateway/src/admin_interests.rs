//! # Gestão admin das áreas de interesse (`interest_area`) — migration 0661.
//!
//! CRUD das áreas temáticas ministeriais (Saúde, Educação, Segurança…) que o cidadão marca no
//! perfil (`citizen_interest`). Até então as 23 áreas eram apenas seedadas; aqui o admin ganha
//! controle para adicionar/editar/remover/reordenar. Admin-gated via header `x-dsoc-citizen-id` +
//! `admin_role_binding`. English identifiers no código; dados (nomes das áreas) em pt-BR.
//! Runtime queries (sem macro `sqlx::query!`).
//!
//! - `GET    /admin/interest-areas`        — lista com contagem de cidadãos por área.
//! - `POST   /admin/interest-areas`        — cria (slug/name/ministry/position).
//! - `PUT    /admin/interest-areas/{slug}` — edita name/ministry/position.
//! - `DELETE /admin/interest-areas/{slug}` — remove (409 se houver cidadão usando).

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/interest-areas", get(list).post(create))
        .route("/admin/interest-areas/{slug}", axum::routing::put(update).delete(remove))
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
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<(), Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "Autenticação necessária."));
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
        Err(fail(StatusCode::FORBIDDEN, "forbidden", "Requer administrador."))
    }
}

// --- List: áreas + contagem de uso ---------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AreaRow {
    slug: String,
    name: String,
    ministry: Option<String>,
    position: i32,
    citizen_count: i64,
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    // LEFT JOIN em citizen_interest para trazer quantos cidadãos usam cada área.
    let rows: Result<Vec<AreaRow>, sqlx::Error> = sqlx::query_as(
        r"SELECT a.slug, a.name, a.ministry, a.position,
                 count(ci.citizen_id)::bigint AS citizen_count
          FROM interest_area a
          LEFT JOIN citizen_interest ci ON ci.area_slug = a.slug
          GROUP BY a.slug, a.name, a.ministry, a.position
          ORDER BY a.position, a.name",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "admin_interests list");
            storage_error()
        }
    }
}

// --- Create --------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateBody {
    slug: String,
    name: String,
    ministry: Option<String>,
    position: Option<i32>,
}

/// Normaliza o slug: minúsculo, sem espaços nas pontas.
fn clean_slug(raw: &str) -> String {
    raw.trim().to_lowercase()
}

async fn create(State(state): State<AppState>, headers: HeaderMap, Json(body): Json<CreateBody>) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let slug = clean_slug(&body.slug);
    let name = body.name.trim().to_string();
    if slug.is_empty() || name.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "invalid_input", "Slug e nome são obrigatórios.");
    }
    let ministry = body.ministry.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    let position = body.position.unwrap_or(0);

    let res = sqlx::query(
        r"INSERT INTO interest_area (slug, name, ministry, position) VALUES ($1, $2, $3, $4)",
    )
    .bind(&slug)
    .bind(&name)
    .bind(&ministry)
    .bind(position)
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(AreaRow { slug, name, ministry, position, citizen_count: 0 })),
        )
            .into_response(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            fail(StatusCode::CONFLICT, "duplicate", "Já existe uma área com esse slug.")
        }
        Err(err) => {
            tracing::error!(?err, "admin_interests create");
            storage_error()
        }
    }
}

// --- Update --------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateBody {
    name: String,
    ministry: Option<String>,
    position: Option<i32>,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let slug = clean_slug(&slug);
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "invalid_input", "Nome é obrigatório.");
    }
    let ministry = body.ministry.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    let position = body.position.unwrap_or(0);

    let res = sqlx::query(
        r"UPDATE interest_area SET name = $2, ministry = $3, position = $4 WHERE slug = $1",
    )
    .bind(&slug)
    .bind(&name)
    .bind(&ministry)
    .bind(position)
    .execute(&state.db)
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => {
            fail(StatusCode::NOT_FOUND, "not_found", "Área não encontrada.")
        }
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(serde_json::json!({ "updated": true }))))
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "admin_interests update");
            storage_error()
        }
    }
}

// --- Delete: só se não houver cidadão usando ----------------------------------------------------

async fn remove(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let slug = clean_slug(&slug);

    // Bloqueia a remoção se algum cidadão ainda tiver esta área marcada (409).
    let in_use: i64 = match sqlx::query_scalar(
        "SELECT count(*) FROM citizen_interest WHERE area_slug = $1",
    )
    .bind(&slug)
    .fetch_one(&state.db)
    .await
    {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(?err, "admin_interests delete count");
            return storage_error();
        }
    };
    if in_use > 0 {
        return fail(
            StatusCode::CONFLICT,
            "in_use",
            "Área em uso por cidadãos; não pode ser removida.",
        );
    }

    let res = sqlx::query("DELETE FROM interest_area WHERE slug = $1")
        .bind(&slug)
        .execute(&state.db)
        .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => {
            fail(StatusCode::NOT_FOUND, "not_found", "Área não encontrada.")
        }
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(serde_json::json!({ "deleted": true }))))
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "admin_interests delete");
            storage_error()
        }
    }
}
