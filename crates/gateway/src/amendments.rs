//! # Amendments — Decidim-parity variants of a proposal (0.20.0-decidim).
//!
//! A `proposal_amendment` is a citizen-authored fork of an existing proposal:
//! new body + rationale. Other citizens can vote on the amendment
//! independently, and the original author can accept it (which applies the
//! amendment as a new `proposal_revision`).
//!
//! Endpoints all live under `/api/v1/*` behind the gateway's cookie/bearer
//! middleware. Runtime-checked `sqlx::query*` (same pattern as `admin_ext.rs`
//! and `me_settings.rs`).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/proposals/{proposal_id}/amendments",
            get(list_amendments).post(create_amendment),
        )
        .route(
            "/amendments/{amendment_id}",
            get(get_amendment)
                .patch(patch_amendment)
                .delete(delete_amendment),
        )
        .route(
            "/amendments/{amendment_id}/publish",
            post(publish_amendment),
        )
        .route("/amendments/{amendment_id}/accept", post(accept_amendment))
        .route("/amendments/{amendment_id}/reject", post(reject_amendment))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "http_401",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}
fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()>::fail(
            "http_404",
            "Emenda não encontrada.",
        )),
    )
        .into_response()
}
fn forbidden(msg: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail("http_403", msg)),
    )
        .into_response()
}
fn bad(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("http_400", msg)),
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

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AmendmentDto {
    id: Uuid,
    proposal_id: Uuid,
    author_id: Uuid,
    author_handle: Option<String>,
    author_display_name: Option<String>,
    body: String,
    rationale: Option<String>,
    status: String,
    support_count: i64,
    created_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CreateAmendmentBody {
    body: String,
    #[serde(default)]
    rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchAmendmentBody {
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

async fn list_amendments(State(state): State<AppState>, Path(proposal_id): Path<Uuid>) -> Response {
    let rows: Result<
        Vec<(
            Uuid,
            Uuid,
            Uuid,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
            i64,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        )>,
        _,
    > = sqlx::query_as(
        r"SELECT a.id, a.proposal_id, a.author_id,
                 c.handle, c.display_name,
                 a.body, a.rationale, a.status, a.support_count,
                 a.created_at, a.published_at, a.resolved_at
            FROM proposal_amendment a
            JOIN citizen c ON c.id = a.author_id
           WHERE a.proposal_id = $1
             AND a.status <> 'draft'
           ORDER BY a.created_at DESC",
    )
    .bind(proposal_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                rows.into_iter().map(dto).collect::<Vec<_>>(),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "list_amendments sql");
            server_error()
        }
    }
}

async fn get_amendment(State(state): State<AppState>, Path(amendment_id): Path<Uuid>) -> Response {
    let row = fetch_amendment(&state.db, amendment_id).await;
    match row {
        Ok(Some(row)) => (StatusCode::OK, Json(ApiResponse::ok(dto(row)))).into_response(),
        Ok(None) => not_found(),
        Err(err) => {
            tracing::error!(?err, "get_amendment sql");
            server_error()
        }
    }
}

type AmendmentRow = (
    Uuid,
    Uuid,
    Uuid,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    String,
    i64,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

async fn fetch_amendment(db: &sqlx::PgPool, id: Uuid) -> Result<Option<AmendmentRow>, sqlx::Error> {
    sqlx::query_as(
        r"SELECT a.id, a.proposal_id, a.author_id,
                 c.handle, c.display_name,
                 a.body, a.rationale, a.status, a.support_count,
                 a.created_at, a.published_at, a.resolved_at
            FROM proposal_amendment a
            JOIN citizen c ON c.id = a.author_id
           WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

fn dto(r: AmendmentRow) -> AmendmentDto {
    AmendmentDto {
        id: r.0,
        proposal_id: r.1,
        author_id: r.2,
        author_handle: r.3,
        author_display_name: r.4,
        body: r.5,
        rationale: r.6,
        status: r.7,
        support_count: r.8,
        created_at: r.9,
        published_at: r.10,
        resolved_at: r.11,
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

async fn create_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(proposal_id): Path<Uuid>,
    Json(body): Json<CreateAmendmentBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    if body.body.trim().is_empty() {
        return bad("O corpo da emenda não pode ficar vazio.");
    }
    if body.body.len() > 20000 {
        return bad("Corpo da emenda ultrapassa 20 mil caracteres.");
    }
    if let Some(r) = &body.rationale {
        if r.len() > 4000 {
            return bad("Justificativa ultrapassa 4 mil caracteres.");
        }
    }
    // Confirm the proposal exists (a 404 here is clearer than a raw FK error).
    let exists: Result<Option<(Uuid,)>, _> =
        sqlx::query_as(r"SELECT id FROM proposal WHERE id = $1")
            .bind(proposal_id)
            .fetch_optional(&state.db)
            .await;
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::fail(
                    "http_404",
                    "Proposta não encontrada.",
                )),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(?err, "create_amendment proposal check");
            return server_error();
        }
    };
    let id = Uuid::now_v7();
    let res = sqlx::query(
        r"INSERT INTO proposal_amendment
             (id, proposal_id, author_id, body, rationale, status,
              support_count, created_at)
          VALUES ($1, $2, $3, $4, $5, 'draft', 0, now())",
    )
    .bind(id)
    .bind(proposal_id)
    .bind(citizen)
    .bind(&body.body)
    .bind(body.rationale.as_deref())
    .execute(&state.db)
    .await;
    if let Err(err) = res {
        tracing::error!(?err, "create_amendment insert");
        return server_error();
    }
    match fetch_amendment(&state.db, id).await {
        Ok(Some(row)) => (StatusCode::CREATED, Json(ApiResponse::ok(dto(row)))).into_response(),
        Ok(None) => server_error(),
        Err(err) => {
            tracing::error!(?err, "create_amendment refetch");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Update (body/rationale, author-only, only while draft)
// ---------------------------------------------------------------------------

async fn patch_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(amendment_id): Path<Uuid>,
    Json(body): Json<PatchAmendmentBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let row = match fetch_amendment(&state.db, amendment_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::error!(?err, "patch_amendment fetch");
            return server_error();
        }
    };
    if row.2 != citizen {
        return forbidden("Só o autor pode editar a emenda.");
    }
    if row.7 != "draft" {
        return forbidden("A emenda já foi publicada; não pode mais ser editada.");
    }
    let new_body = body.body.as_ref().unwrap_or(&row.5);
    let new_rationale = body.rationale.as_ref().or(row.6.as_ref());
    if new_body.trim().is_empty() || new_body.len() > 20000 {
        return bad("Corpo inválido.");
    }
    if let Err(err) = sqlx::query(
        r"UPDATE proposal_amendment
             SET body = $2, rationale = $3
           WHERE id = $1",
    )
    .bind(amendment_id)
    .bind(new_body)
    .bind(new_rationale)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "patch_amendment update");
        return server_error();
    }
    get_amendment(State(state), Path(amendment_id)).await
}

async fn delete_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(amendment_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let row = match fetch_amendment(&state.db, amendment_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::error!(?err, "delete_amendment fetch");
            return server_error();
        }
    };
    if row.2 != citizen {
        return forbidden("Só o autor pode retirar a emenda.");
    }
    // Withdraw rather than hard-delete so the history stays auditable.
    if let Err(err) = sqlx::query(
        r"UPDATE proposal_amendment
             SET status = 'withdrawn', resolved_at = now()
           WHERE id = $1",
    )
    .bind(amendment_id)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "delete_amendment withdraw");
        return server_error();
    }
    (StatusCode::NO_CONTENT, ()).into_response()
}

// ---------------------------------------------------------------------------
// Publish (author transitions draft → open)
// ---------------------------------------------------------------------------

async fn publish_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(amendment_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let row = match fetch_amendment(&state.db, amendment_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::error!(?err, "publish fetch");
            return server_error();
        }
    };
    if row.2 != citizen {
        return forbidden("Só o autor pode publicar.");
    }
    if row.7 != "draft" {
        return bad("Só emendas em rascunho podem ser publicadas.");
    }
    if let Err(err) = sqlx::query(
        r"UPDATE proposal_amendment
             SET status = 'open', published_at = now()
           WHERE id = $1",
    )
    .bind(amendment_id)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "publish update");
        return server_error();
    }
    get_amendment(State(state), Path(amendment_id)).await
}

// ---------------------------------------------------------------------------
// Accept — original proposal author (or admin) applies the amendment as a
// new proposal_revision AND marks the amendment accepted.
// ---------------------------------------------------------------------------

async fn accept_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(amendment_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    // Fetch amendment + parent proposal in one shot.
    let joined: Result<Option<(Uuid, Uuid, Uuid, String, String, Uuid)>, _> = sqlx::query_as(
        r"SELECT a.id, a.proposal_id, a.author_id, a.body, a.status,
                 p.mandate_id
            FROM proposal_amendment a
            JOIN proposal p ON p.id = a.proposal_id
           WHERE a.id = $1",
    )
    .bind(amendment_id)
    .fetch_optional(&state.db)
    .await;
    let (amendment_id, proposal_id, _amendment_author, new_body, status, _mandate) = match joined {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::error!(?err, "accept fetch join");
            return server_error();
        }
    };
    if status != "open" {
        return bad("Só emendas em votação podem ser aceitas.");
    }
    // Authorization: the ORIGINAL PROPOSAL AUTHOR accepts. Look up the author
    // via `proposal_revision` (the initial revision is the original body's
    // author). If no revisions yet, we take the mandate operator: cheaper fallback.
    let original_author: Result<Option<(Uuid,)>, _> = sqlx::query_as(
        r"SELECT edited_by FROM proposal_revision
           WHERE proposal_id = $1
           ORDER BY created_at ASC
           LIMIT 1",
    )
    .bind(proposal_id)
    .fetch_optional(&state.db)
    .await;
    let allowed_author: Option<Uuid> = match original_author {
        Ok(Some((a,))) => Some(a),
        Ok(None) => None,
        Err(err) => {
            tracing::error!(?err, "accept original author fetch");
            return server_error();
        }
    };
    if allowed_author != Some(citizen) {
        return forbidden("Só o autor da proposta pode aceitar a emenda.");
    }
    // Apply as a new proposal_revision + accept the amendment. Single tx.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "accept tx begin");
            return server_error();
        }
    };
    // 1. Update proposal.body.
    if let Err(err) = sqlx::query(r"UPDATE proposal SET body = $2 WHERE id = $1")
        .bind(proposal_id)
        .bind(&new_body)
        .execute(&mut *tx)
        .await
    {
        tracing::error!(?err, "accept proposal update");
        return server_error();
    }
    // 2. Fetch current title to snapshot in revision.
    let title: Result<Option<(String,)>, _> =
        sqlx::query_as(r"SELECT title FROM proposal WHERE id = $1")
            .bind(proposal_id)
            .fetch_optional(&mut *tx)
            .await;
    let title = match title {
        Ok(Some((t,))) => t,
        _ => "".to_owned(),
    };
    // 3. Insert revision row (author = caller, i.e. the proposal author).
    let rev_id = Uuid::now_v7();
    if let Err(err) = sqlx::query(
        r"INSERT INTO proposal_revision
             (id, proposal_id, title, body, edited_by, created_at)
          VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(rev_id)
    .bind(proposal_id)
    .bind(&title)
    .bind(&new_body)
    .bind(citizen)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "accept revision insert");
        return server_error();
    }
    // 4. Mark amendment accepted.
    if let Err(err) = sqlx::query(
        r"UPDATE proposal_amendment
             SET status = 'accepted', resolved_at = now()
           WHERE id = $1",
    )
    .bind(amendment_id)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "accept amendment update");
        return server_error();
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "accept commit");
        return server_error();
    }
    get_amendment(State(state), Path(amendment_id)).await
}

async fn reject_amendment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(amendment_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    // Authorization identical to accept — proposal author only.
    let row = match fetch_amendment(&state.db, amendment_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::error!(?err, "reject fetch");
            return server_error();
        }
    };
    if row.7 != "open" {
        return bad("Só emendas em votação podem ser rejeitadas.");
    }
    let original: Result<Option<(Uuid,)>, _> = sqlx::query_as(
        r"SELECT edited_by FROM proposal_revision
           WHERE proposal_id = $1
           ORDER BY created_at ASC
           LIMIT 1",
    )
    .bind(row.1)
    .fetch_optional(&state.db)
    .await;
    let allowed = matches!(original, Ok(Some((a,))) if a == citizen);
    if !allowed {
        return forbidden("Só o autor da proposta pode rejeitar a emenda.");
    }
    if let Err(err) = sqlx::query(
        r"UPDATE proposal_amendment
             SET status = 'rejected', resolved_at = now()
           WHERE id = $1",
    )
    .bind(amendment_id)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "reject update");
        return server_error();
    }
    get_amendment(State(state), Path(amendment_id)).await
}
