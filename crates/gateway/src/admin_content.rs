//! # Super-admin: content editing and removal (0.40.0) — SOCRATES.
//!
//! The owner/admin (SOCRATES) lacked the power to curate the catalog: editing an
//! official/party and deleting a proposal/official. "Delete" HIDES by default
//! (reversible, migration 0528); a `?force=true` performs the cascading hard delete.
//!
//! Gate: an `admin_role_binding` with role owner/admin (the same criterion as `admin_ext`).
//!
//! - `PATCH  /admin/mandates/{id}`        — edita campos do mandato.
//! - `POST   /admin/mandates/{id}/hide`   — oculta / `?on=false` reexibe.
//! - `DELETE /admin/mandates/{id}?force=true` — hard-delete em cascata.
//! - `POST   /admin/proposals/{id}/hide`  — oculta / reexibe.
//! - `DELETE /admin/proposals/{id}?force=true` — hard-delete.
//! - `PATCH  /admin/parties/{sigla}`      — edit name/number/logo.
//! - `DELETE /admin/parties/{sigla}?force=true` — delete (cascading over directories/admins).

use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, patch, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/mandates/{id}",
            patch(edit_mandate).delete(delete_mandate),
        )
        .route("/admin/mandates/{id}/hide", post(hide_mandate))
        .route("/admin/proposals/{id}", delete(delete_proposal))
        .route("/admin/proposals/{id}/hide", post(hide_proposal))
        .route(
            "/admin/parties/{sigla}",
            patch(edit_party).delete(delete_party),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn ok_empty() -> Response {
    (StatusCode::OK, Json(ApiResponse::ok(()))).into_response()
}

fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage",
        "Erro interno.",
    )
}

/// Owner/admin gate (SOCRATES). Returns Err(a ready response) when it does not pass.
/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8).
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
}

#[derive(Debug, Deserialize)]
struct HideParams {
    /// `on=false` reexibe; ausente/true oculta.
    on: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ForceParams {
    force: Option<bool>,
}

// ---------------------------------------------------------------------------
// Mandate — editar
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EditMandateBody {
    display_name: Option<String>,
    party: Option<String>,
    office: Option<String>,
    uf: Option<String>,
    municipio: Option<String>,
    house: Option<String>,
    sphere: Option<String>,
    public_email: Option<String>,
}

async fn edit_mandate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<EditMandateBody>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    // COALESCE: only overwrite what arrived in the body (omitted fields stay as they are).
    // Strings vazias viram NULL nos campos territoriais (limpa uf/municipio).
    let norm = |s: Option<String>| s.map(|v| v.trim().to_owned());
    let uf = norm(b.uf).map(|v| v.to_uppercase());
    if let Some(u) = &uf {
        if !u.is_empty() && (u.len() != 2 || !u.chars().all(|c| c.is_ascii_alphabetic())) {
            return fail(StatusCode::BAD_REQUEST, "invalid_uf", "UF inválida.");
        }
    }
    let res = sqlx::query(
        r"UPDATE mandate SET
            display_name = COALESCE($2, display_name),
            party        = CASE WHEN $3::text IS NULL THEN party ELSE NULLIF($3,'') END,
            office       = COALESCE(NULLIF($4,''), office),
            uf           = CASE WHEN $5::text IS NULL THEN uf ELSE NULLIF($5,'') END,
            municipio    = CASE WHEN $6::text IS NULL THEN municipio ELSE NULLIF($6,'') END,
            house        = CASE WHEN $7::text IS NULL THEN house ELSE NULLIF($7,'') END,
            sphere       = COALESCE(NULLIF($8,''), sphere),
            public_email = COALESCE(NULLIF($9,''), public_email)
          WHERE id = $1",
    )
    .bind(id)
    .bind(norm(b.display_name).filter(|s| !s.is_empty()))
    .bind(norm(b.party))
    .bind(norm(b.office))
    .bind(uf)
    .bind(norm(b.municipio))
    .bind(norm(b.house))
    .bind(norm(b.sphere))
    .bind(norm(b.public_email))
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Mandato não encontrado.",
        ),
        Ok(_) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin edit_mandate");
            storage_error()
        }
    }
}

async fn hide_mandate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(p): Query<HideParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let hide = p.on.unwrap_or(true);
    let res = sqlx::query(
        "UPDATE mandate SET hidden_at = CASE WHEN $2 THEN now() ELSE NULL END WHERE id = $1",
    )
    .bind(id)
    .bind(hide)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Mandato não encontrado.",
        ),
        Ok(_) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin hide_mandate");
            storage_error()
        }
    }
}

async fn delete_mandate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(p): Query<ForceParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    if p.force != Some(true) {
        return fail(
            StatusCode::BAD_REQUEST,
            "force_required",
            "Hard-delete exige ?force=true. Use ocultar para remover de forma reversível.",
        );
    }
    // Cascade in one tx: any FK left behind rolls back with an error (never corrupts).
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "admin delete_mandate: begin");
            return storage_error();
        }
    };
    let steps: &[&str] = &[
        // the mandate's proposals (revisions first; amendments cascade on their own).
        "DELETE FROM proposal_revision WHERE proposal_id IN (SELECT id FROM proposal WHERE mandate_id = $1)",
        "DELETE FROM proposal WHERE mandate_id = $1",
        "DELETE FROM consequence_response WHERE mandate_id = $1",
        "DELETE FROM consequence_sla WHERE mandate_id = $1",
        "DELETE FROM scorecard WHERE mandate_id = $1",
        "DELETE FROM notification_receipt WHERE mandate_id = $1",
        "DELETE FROM mandate_invite WHERE mandate_id = $1",
        "DELETE FROM mandate_invitation WHERE mandate_id = $1",
        "DELETE FROM mandate_office WHERE mandate_id = $1",
        "DELETE FROM mandate_identity_binding WHERE mandate_id = $1",
        "DELETE FROM campaign_group WHERE mandate_id = $1",
        // candidacy is nullable — preserve the candidacy, only drop the binding.
        "UPDATE candidacy SET mandate_id = NULL WHERE mandate_id = $1",
        "DELETE FROM mandate WHERE id = $1",
    ];
    for sql in steps {
        if let Err(err) = sqlx::query(sql).bind(id).execute(&mut *tx).await {
            tracing::error!(?err, sql, "admin delete_mandate: step");
            let _ = tx.rollback().await;
            return fail(
                StatusCode::CONFLICT,
                "has_dependencies",
                "Não foi possível apagar (há dependências não previstas). Use ocultar.",
            );
        }
    }
    match tx.commit().await {
        Ok(()) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin delete_mandate: commit");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Proposal — ocultar / apagar
// ---------------------------------------------------------------------------

async fn hide_proposal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(p): Query<HideParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let hide = p.on.unwrap_or(true);
    let res = sqlx::query(
        "UPDATE proposal SET hidden_at = CASE WHEN $2 THEN now() ELSE NULL END WHERE id = $1",
    )
    .bind(id)
    .bind(hide)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Proposta não encontrada.",
        ),
        Ok(_) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin hide_proposal");
            storage_error()
        }
    }
}

async fn delete_proposal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(p): Query<ForceParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    if p.force != Some(true) {
        return fail(
            StatusCode::BAD_REQUEST,
            "force_required",
            "Hard-delete exige ?force=true. Use ocultar para remover de forma reversível.",
        );
    }
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "admin delete_proposal: begin");
            return storage_error();
        }
    };
    for sql in [
        "DELETE FROM proposal_revision WHERE proposal_id = $1",
        "DELETE FROM proposal WHERE id = $1",
    ] {
        if let Err(err) = sqlx::query(sql).bind(id).execute(&mut *tx).await {
            tracing::error!(?err, sql, "admin delete_proposal: step");
            let _ = tx.rollback().await;
            return fail(
                StatusCode::CONFLICT,
                "has_dependencies",
                "Não foi possível apagar.",
            );
        }
    }
    match tx.commit().await {
        Ok(()) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin delete_proposal: commit");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Party — editar / apagar
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EditPartyBody {
    name: Option<String>,
    tse_number: Option<i32>,
    logo_url: Option<String>,
    website: Option<String>,
    founded_year: Option<i32>,
}

async fn edit_party(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(sigla): Path<String>,
    Json(b): Json<EditPartyBody>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let name = b
        .name
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let logo = b.logo_url.map(|s| s.trim().to_owned());
    let website = b.website.map(|s| s.trim().to_owned());
    let res = sqlx::query(
        r"UPDATE party SET
            name         = COALESCE($3, name),
            tse_number   = COALESCE($4, tse_number),
            logo_url     = CASE WHEN $5::text IS NULL THEN logo_url ELSE NULLIF($5,'') END,
            founded_year = COALESCE($6, founded_year),
            website      = CASE WHEN $7::text IS NULL THEN website ELSE NULLIF($7,'') END
          WHERE org_id = $1 AND sigla = $2",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(&sigla)
    .bind(name)
    .bind(b.tse_number)
    .bind(logo)
    .bind(b.founded_year)
    .bind(website)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Partido não encontrado.",
        ),
        Ok(_) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin edit_party");
            storage_error()
        }
    }
}

async fn delete_party(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(sigla): Path<String>,
    Query(p): Query<ForceParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    if p.force != Some(true) {
        return fail(
            StatusCode::BAD_REQUEST,
            "force_required",
            "Apagar partido exige ?force=true (remove diretórios e admins do partido).",
        );
    }
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "admin delete_party: begin");
            return storage_error();
        }
    };
    for sql in [
        "DELETE FROM party_administrator WHERE org_id = $1 AND party_sigla = $2",
        "DELETE FROM party_directory WHERE org_id = $1 AND party_sigla = $2",
        "DELETE FROM party WHERE org_id = $1 AND sigla = $2",
    ] {
        if let Err(err) = sqlx::query(sql)
            .bind(DEFAULT_ORG_UUID)
            .bind(&sigla)
            .execute(&mut *tx)
            .await
        {
            tracing::error!(?err, sql, "admin delete_party: step");
            let _ = tx.rollback().await;
            return fail(
                StatusCode::CONFLICT,
                "has_dependencies",
                "Não foi possível apagar o partido.",
            );
        }
    }
    match tx.commit().await {
        Ok(()) => ok_empty(),
        Err(err) => {
            tracing::error!(?err, "admin delete_party: commit");
            storage_error()
        }
    }
}
