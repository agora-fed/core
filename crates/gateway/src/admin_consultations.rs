//! # Admin management of consultations — the list/detail/close panel.
//!
//! The consultations subsystem (`dsoc-consultations`, migration 0531) already exists and the
//! campaign broadcast (`campaign_broadcast.rs`) creates micro-consultations, but the admin had no
//! screen to **list/detail/close** them. This module covers that, following the pattern of
//! `civic_sources.rs`: admin-gated via the `x-dsoc-citizen-id` header + `admin_role_binding`, with
//! **runtime queries** (sqlx::query/query_as — without the `sqlx::query!` macro's cache).
//!
//! The `status` vocabulary is identical to the schema (`ConsultationStatus`): 'open' and 'closed'.
//!
//! - `GET  /admin/consultations` — a paginated list (title, status, window, question count,
//!   answer count). Filters: status, q (title search).
//! - `GET  /admin/consultations/{id}`       — detail + questions (with an aggregate per answer).
//! - `POST /admin/consultations/{id}/close` — encerra (status→'closed'), idempotente.

use axum::extract::{Path, Query, State};
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

/// The installation's default org (Pindorama) when the header is absent.
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

pub(crate) fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/consultations", get(list))
        .route("/admin/consultations/{id}", get(detail))
        .route("/admin/consultations/{id}/close", post(close))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// The caller's org (a header injected by the session auth), falling back to the default org.
fn caller_org(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
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

/// Requires a platform admin (owner/admin) in the caller's org. Returns the resolved org.
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        ));
    };
    let org = caller_org(headers);
    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admin_role_binding \
         WHERE org_id=$1 AND citizen_id=$2 AND role IN ('owner','admin'))",
    )
    .bind(org)
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if is_admin {
        Ok(org)
    } else {
        Err(fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Requer administrador.",
        ))
    }
}

// --- Lista: paginada + filtrada ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    /// Filter by status ('open' | 'closed').
    status: Option<String>,
    /// Search (ILIKE) in the title.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ConsultationRow {
    id: Uuid,
    title: String,
    status: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    question_count: i64,
    response_count: i64,
}

#[derive(Debug, Serialize)]
struct ListResult {
    total: i64,
    limit: i64,
    offset: i64,
    items: Vec<ConsultationRow>,
}

/// WHERE clause shared by the count and the listing (org + optional filters).
fn push_where(
    qb: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    org: Uuid,
    status: &Option<String>,
    q: &Option<String>,
) {
    qb.push(" WHERE c.org_id = ").push_bind(org);
    if let Some(s) = status {
        qb.push(" AND c.status = ").push_bind(s.clone());
    }
    if let Some(pat) = q {
        qb.push(" AND c.title ILIKE ").push_bind(pat.clone());
    }
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    let org = match require_admin(&state.db, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    let offset = p.offset.unwrap_or(0).max(0);
    let q = p.q.map(|s| format!("%{}%", s.trim()));
    let status = p.status.filter(|s| !s.is_empty());

    let mut cb = sqlx::QueryBuilder::new("SELECT count(*) FROM consultations_consultation c");
    push_where(&mut cb, org, &status, &q);
    let total: i64 = match cb.build_query_scalar().fetch_one(&state.db).await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "admin_consultations count");
            return storage_error();
        }
    };

    // Counts by subquery: number of questions and number of answers (answers link to the
    // consultation through the question, since consultation_response references question_id).
    let mut lb = sqlx::QueryBuilder::new(
        "SELECT c.id, c.title, c.status, c.opens_at, c.closes_at, c.created_at, \
         (SELECT count(*) FROM consultations_consultation_question q \
            WHERE q.consultation_id = c.id) AS question_count, \
         (SELECT count(*) FROM consultation_response r \
            JOIN consultations_consultation_question q ON q.id = r.question_id \
            WHERE q.consultation_id = c.id) AS response_count \
         FROM consultations_consultation c",
    );
    push_where(&mut lb, org, &status, &q);
    lb.push(" ORDER BY c.created_at DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let items: Vec<ConsultationRow> = match lb.build_query_as().fetch_all(&state.db).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "admin_consultations list");
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

// --- Detail: consultation + questions with aggregate ---------------------------------------------

#[derive(Debug, Serialize)]
struct QuestionDetail {
    id: Uuid,
    prompt: String,
    position: i32,
    concordo: i64,
    neutro: i64,
    discordo: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
struct ConsultationDetail {
    id: Uuid,
    title: String,
    status: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    questions: Vec<QuestionDetail>,
}

async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let org = match require_admin(&state.db, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };

    // The consultation's header, scoped by org.
    let head: Option<(
        Uuid,
        String,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = match sqlx::query_as(
        r"SELECT id, title, status, opens_at, closes_at, created_at
                FROM consultations_consultation
               WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "admin_consultations detail head");
            return storage_error();
        }
    };
    let Some((cid, title, status, opens_at, closes_at, created_at)) = head else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta não encontrada.",
        );
    };

    // Questions ordered by position.
    let questions: Vec<(Uuid, String, i32)> = match sqlx::query_as(
        r"SELECT id, prompt, position
            FROM consultations_consultation_question
           WHERE consultation_id = $1 ORDER BY position ASC",
    )
    .bind(cid)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "admin_consultations detail questions");
            return storage_error();
        }
    };
    let qids: Vec<Uuid> = questions.iter().map(|(qid, _, _)| *qid).collect();

    // Aggregate by (question, answer) in a single SELECT.
    let tallies: Vec<(Uuid, String, i64)> = match sqlx::query_as(
        r"SELECT question_id, answer, count(*)
            FROM consultation_response
           WHERE question_id = ANY($1)
           GROUP BY question_id, answer",
    )
    .bind(&qids)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "admin_consultations detail tally");
            return storage_error();
        }
    };
    let mut by_q: std::collections::HashMap<Uuid, (i64, i64, i64)> =
        std::collections::HashMap::new();
    for (qid, answer, n) in tallies {
        let e = by_q.entry(qid).or_insert((0, 0, 0));
        match answer.as_str() {
            "concordo" => e.0 += n,
            "neutro" => e.1 += n,
            "discordo" => e.2 += n,
            _ => {}
        }
    }

    let questions = questions
        .into_iter()
        .map(|(qid, prompt, position)| {
            let (c, n, d) = by_q.get(&qid).copied().unwrap_or((0, 0, 0));
            QuestionDetail {
                id: qid,
                prompt,
                position,
                concordo: c,
                neutro: n,
                discordo: d,
                total: c + n + d,
            }
        })
        .collect();

    let out = ConsultationDetail {
        id: cid,
        title,
        status,
        opens_at,
        closes_at,
        created_at,
        questions,
    };
    (StatusCode::OK, axum::Json(ApiResponse::ok(out))).into_response()
}

// --- Encerrar: status → 'closed' -----------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CloseResult {
    id: Uuid,
    status: String,
}

async fn close(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let org = match require_admin(&state.db, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };

    // The `status = 'open'` clause makes the UPDATE naturally idempotent and prevents reopening a
    // consultation that is already closed (the domain has no reopen).
    let updated: Option<Uuid> = match sqlx::query_scalar(
        r"UPDATE consultations_consultation
             SET status = 'closed'
           WHERE id = $1 AND org_id = $2 AND status = 'open'
           RETURNING id",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "admin_consultations close");
            return storage_error();
        }
    };

    if updated.is_some() {
        return (
            StatusCode::OK,
            axum::Json(ApiResponse::ok(CloseResult {
                id,
                status: "closed".into(),
            })),
        )
            .into_response();
    }

    // Nothing updated: it either does not exist (in the org) or was already closed. Distinguish both.
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT status FROM consultations_consultation WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    match exists {
        // Already closed: return an idempotent OK with the current status.
        Some(status) => (
            StatusCode::OK,
            axum::Json(ApiResponse::ok(CloseResult { id, status })),
        )
            .into_response(),
        None => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta não encontrada.",
        ),
    }
}
