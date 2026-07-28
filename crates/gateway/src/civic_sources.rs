//! # Catálogo de fontes cívicas (#72, ADR-0017) — painel admin do mapa município→plataforma.
//!
//! Onde o admin vê QUAL sistema cada câmara municipal roda (SAPL/Interlegis, camaraonline…), a
//! URL-base e o status do probe. Alimenta a estratégia de extração por-plataforma. Lê a tabela
//! `civic_source` (migration 0662). Paginado + filtrado. Runtime queries.
//!
//! - `GET /admin/civic-sources/overview` — contagem por plataforma × status do probe.
//! - `GET /admin/civic-sources`          — lista paginada (filtros uf/plataforma/status/q).

use axum::extract::{Query, State};
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
        .route("/admin/civic-sources/overview", get(overview))
        .route("/admin/civic-sources", get(list))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}
fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, axum::Json(ApiResponse::<()>::fail(code, msg))).into_response()
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

// --- Overview: plataforma × status ---------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct OverviewRow {
    platform: String,
    probe_status: String,
    total: i64,
    parlamentares: Option<i64>,
}

async fn overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let rows: Result<Vec<OverviewRow>, sqlx::Error> = sqlx::query_as(
        r"SELECT platform, probe_status, count(*) AS total,
                 sum(parlamentares_found)::bigint AS parlamentares
          FROM civic_source GROUP BY platform, probe_status ORDER BY total DESC",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => (StatusCode::OK, axum::Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "civic_sources overview");
            storage_error()
        }
    }
}

// --- List: paginado + filtrado -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    uf: Option<String>,
    platform: Option<String>,
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct SourceRow {
    id: Uuid,
    uf: String,
    municipio: String,
    platform: String,
    base_url: Option<String>,
    probe_status: String,
    parlamentares_found: Option<i32>,
    last_probed_at: Option<chrono::DateTime<chrono::Utc>>,
    last_extracted_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
struct ListResult {
    total: i64,
    limit: i64,
    offset: i64,
    items: Vec<SourceRow>,
}

fn push_where(
    qb: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    uf: &Option<String>,
    platform: &Option<String>,
    status: &Option<String>,
    q: &Option<String>,
) {
    qb.push(" WHERE 1=1");
    if let Some(u) = uf {
        qb.push(" AND uf = ").push_bind(u.clone());
    }
    if let Some(p) = platform {
        qb.push(" AND platform = ").push_bind(p.clone());
    }
    if let Some(s) = status {
        qb.push(" AND probe_status = ").push_bind(s.clone());
    }
    if let Some(pat) = q {
        qb.push(" AND municipio ILIKE ").push_bind(pat.clone());
    }
}

async fn list(State(state): State<AppState>, headers: HeaderMap, Query(p): Query<ListParams>) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    let offset = p.offset.unwrap_or(0).max(0);
    let q = p.q.map(|s| format!("%{}%", s.trim()));
    let uf = p.uf.map(|s| s.to_uppercase());

    let mut cb = sqlx::QueryBuilder::new("SELECT count(*) FROM civic_source");
    push_where(&mut cb, &uf, &p.platform, &p.status, &q);
    let total: i64 = match cb.build_query_scalar().fetch_one(&state.db).await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "civic_sources count");
            return storage_error();
        }
    };

    let mut lb = sqlx::QueryBuilder::new(
        "SELECT id, uf, municipio, platform, base_url, probe_status, parlamentares_found, \
         last_probed_at, last_extracted_at FROM civic_source",
    );
    push_where(&mut lb, &uf, &p.platform, &p.status, &q);
    lb.push(" ORDER BY uf, municipio LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let items: Vec<SourceRow> = match lb.build_query_as().fetch_all(&state.db).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "civic_sources list");
            return storage_error();
        }
    };
    (StatusCode::OK, axum::Json(ApiResponse::ok(ListResult { total, limit, offset, items }))).into_response()
}
