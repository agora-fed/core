//! # Officials' contacts (0.51.0) — the admin e-mail audit panel.
//!
//! Where the admin sees EACH mandate's e-mail (with a real vs placeholder badge), to
//! know who is reachable by proposal/invitation. Paginated + filtered (it never pulls
//! the ~70k at once). Runtime queries.
//!
//! - `GET /admin/politico-contacts/overview`  — matriz cargo × (real / placeholder).
//! - `GET /admin/politico-contacts`           — lista paginada, filtros cargo/uf/status/q.

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

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
const PLACEHOLDER: &str = "%@parlamento.democracia.social.br";

pub(crate) fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/politico-contacts/overview", get(overview))
        .route("/admin/politico-contacts", get(list))
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

/// A FIXED SQL fragment (safe, never user-supplied) for the office filter.
fn cargo_clause(cargo: &str) -> Option<&'static str> {
    match cargo {
        "vereador" => Some("office ILIKE 'Vereador%'"),
        "dep_estadual" => {
            Some("(office ILIKE 'Deputado(a) Estadual%' OR office ILIKE 'Deputado(a) Distrital%')")
        }
        "dep_federal" => Some("office ILIKE 'Deputado(a) Federal%'"),
        "senador" => Some("office ILIKE 'Senador%'"),
        "governador" => Some("office ILIKE 'Governador%'"),
        "prefeito" => Some("(office ILIKE 'Prefeito%' OR office ILIKE 'Vice-Prefeito%')"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct OverviewRow {
    cargo: String,
    total: i64,
    com_email: i64,
    placeholder: i64,
}

async fn overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let rows: Result<Vec<OverviewRow>, sqlx::Error> = sqlx::query_as(
        r"SELECT
            CASE WHEN office ILIKE 'Vereador%' THEN 'vereador'
                 WHEN office ILIKE 'Deputado(a) Estadual%' OR office ILIKE 'Deputado(a) Distrital%' THEN 'dep_estadual'
                 WHEN office ILIKE 'Deputado(a) Federal%' THEN 'dep_federal'
                 WHEN office ILIKE 'Senador%' THEN 'senador'
                 WHEN office ILIKE 'Governador%' THEN 'governador'
                 WHEN office ILIKE 'Prefeito%' OR office ILIKE 'Vice-Prefeito%' THEN 'prefeito'
                 ELSE 'outro' END AS cargo,
            count(*) AS total,
            count(*) FILTER (WHERE public_email NOT ILIKE $2) AS com_email,
            count(*) FILTER (WHERE public_email ILIKE $2) AS placeholder
          FROM mandate WHERE org_id = $1 AND hidden_at IS NULL
          GROUP BY 1 ORDER BY 2 DESC",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(PLACEHOLDER)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => (StatusCode::OK, axum::Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "politico_contacts overview");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// List (paginated, filtered)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    cargo: Option<String>,
    uf: Option<String>,
    status: Option<String>, // real | placeholder | all
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ContactRow {
    id: Uuid,
    display_name: String,
    office: String,
    party: Option<String>,
    uf: Option<String>,
    municipio: Option<String>,
    public_email: String,
    email_real: bool,
}

#[derive(Debug, Serialize)]
struct ListResult {
    total: i64,
    limit: i64,
    offset: i64,
    items: Vec<ContactRow>,
}

/// Apply the WHERE (same filters) on a builder. Called twice (count + page) —
/// each builder receives its own binds.
fn push_where(
    qb: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    cargo: Option<&'static str>,
    uf: &Option<String>,
    status: Option<&str>,
    q: &Option<String>,
) {
    qb.push(" WHERE org_id = ")
        .push_bind(DEFAULT_ORG_UUID)
        .push(" AND hidden_at IS NULL");
    if let Some(c) = cargo {
        qb.push(" AND ").push(c);
    }
    if let Some(u) = uf {
        qb.push(" AND uf = ").push_bind(u.clone());
    }
    match status {
        Some("real") => {
            qb.push(" AND public_email NOT ILIKE ")
                .push_bind(PLACEHOLDER);
        }
        Some("placeholder") => {
            qb.push(" AND public_email ILIKE ").push_bind(PLACEHOLDER);
        }
        _ => {}
    }
    if let Some(pat) = q {
        qb.push(" AND (display_name ILIKE ")
            .push_bind(pat.clone())
            .push(" OR municipio ILIKE ")
            .push_bind(pat.clone())
            .push(" OR office ILIKE ")
            .push_bind(pat.clone())
            .push(")");
    }
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    let offset = p.offset.unwrap_or(0).max(0);
    let uf =
        p.uf.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_uppercase());
    let q =
        p.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));
    let cargo = p
        .cargo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(cargo_clause);
    let status = p.status.as_deref();

    // total
    let mut cb = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM mandate");
    push_where(&mut cb, cargo, &uf, status, &q);
    let total: i64 = match cb.build_query_scalar().fetch_one(&state.db).await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "politico_contacts count");
            return storage_error();
        }
    };

    // page
    let mut lb = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "SELECT id, display_name, office, party, uf, municipio, public_email, (public_email NOT ILIKE ",
    );
    lb.push_bind(PLACEHOLDER)
        .push(") AS email_real FROM mandate");
    push_where(&mut lb, cargo, &uf, status, &q);
    lb.push(" ORDER BY office, display_name LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let items: Vec<ContactRow> = match lb.build_query_as().fetch_all(&state.db).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "politico_contacts list");
            return storage_error();
        }
    };

    (
        StatusCode::OK,
        axum::Json(ApiResponse::ok(ListResult {
            total,
            limit,
            offset,
            items,
        })),
    )
        .into_response()
}
