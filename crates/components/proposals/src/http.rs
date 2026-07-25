//! Axum surface for `proposals`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004). Every **mutating**
//! handler authorizes the authenticated caller against the org via the injected
//! `Arc<dyn Authorization>` before any write; the caller is never trusted from the body unchecked.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta, ProposalDto, ProposalTargetDto};
use dsoc_core::ids::{MandateId, OrgId, ProposalId};
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{NewProposal, NewRevision};
use crate::queries::{ProposalRow, RevisionRow};
use crate::service::ProposalService;

/// Request to create a proposal. The acting citizen and org come from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body; `mandate_id` is the office the proposal is
/// directed at.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateProposalRequest {
    /// The mandate this proposal is directed at (the PRINCIPAL target).
    pub mandate_id: Uuid,
    /// Co-destinatários (0537 — multi-gabinete): mandatos adicionais que também recebem a
    /// proposta. Devem existir e ser da MESMA esfera federativa do principal; duplicatas são
    /// ignoradas; total (principal incluído) limitado a
    /// [`crate::domain::MAX_PROPOSAL_TARGETS`]. Aditivo: clientes velhos omitem o campo.
    #[serde(default)]
    pub additional_mandate_ids: Vec<Uuid>,
    /// Title (Portuguese civic content).
    pub title: String,
    /// Body / description.
    pub body: String,
    /// Support count at which the consequence loop fires (must be positive).
    pub threshold: i32,
}

/// Request to append a revision to a proposal. The author and org come from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateRevisionRequest {
    /// New title.
    pub title: String,
    /// New body.
    pub body: String,
}

/// Public view of an append-only revision.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RevisionDto {
    /// Revision id.
    pub id: Uuid,
    /// Proposal this revision belongs to.
    pub proposal_id: Uuid,
    /// Title at this revision.
    pub title: String,
    /// Body at this revision.
    pub body: String,
    /// Citizen who authored this revision.
    pub edited_by: Uuid,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<RevisionRow> for RevisionDto {
    fn from(row: RevisionRow) -> Self {
        Self {
            id: row.id,
            proposal_id: row.proposal_id,
            title: row.title,
            body: row.body,
            edited_by: row.edited_by,
            created_at: row.created_at,
        }
    }
}

/// Map an internal proposal row onto the shared public DTO. Avatar URL is composed from
/// `MEDIA_BASE_URL` (or `/media` for same-origin default) + the stored object key — same
/// convention the mandate DTO uses. The `author_public_handle` always travels when there IS an
/// author so the UI can fall back to "u-xxxx" if the citizen never picked a `@handle`.
fn proposal_dto(row: ProposalRow, targets: Vec<crate::queries::TargetRow>) -> ProposalDto {
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let media_base = media_base.trim_end_matches('/').to_owned();
    let author_avatar_url = row
        .author_avatar_object_key
        .as_ref()
        .map(|k| format!("{media_base}/{k}"));
    let author_public_handle = row.author_citizen_id.map(|cid| {
        // Mirrors the format `dsoc_auth::domain::public_handle` emits — `u-` + simple uuid.
        format!("u-{}", cid.as_simple())
    });
    ProposalDto {
        id: row.id,
        title: row.title,
        body: row.body,
        mandate_id: row.mandate_id,
        cluster_id: row.cluster_id,
        support_count: u64::try_from(row.support_count).unwrap_or(0),
        threshold: u64::try_from(row.threshold).unwrap_or(0),
        author_handle: row.author_handle,
        author_public_handle,
        author_avatar_url,
        urgencia: row.urgencia,
        status: row.status,
        published_at: row.published_at,
        notified_author_at: row.notified_author_at,
        notified_mandate_at: row.notified_mandate_at,
        targets: targets
            .into_iter()
            .map(|t| ProposalTargetDto {
                mandate_id: t.mandate_id,
                display_name: t.display_name,
                office: t.office,
                sphere: t.sphere,
                notified_at: t.notified_at,
            })
            .collect(),
        created_at: row.created_at,
    }
}

/// Query parameters for listing proposals (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return proposals with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// Query parameters for listing a proposal's revisions.
#[derive(Debug, Clone, Deserialize)]
pub struct RevisionListParams {
    /// Keyset cursor over ascending revision id. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: dsoc_app::AppState) -> Router<()> {
    Router::new()
        .route("/proposals", post(create).get(list))
        .route("/proposals/{id}", get(get_one))
        .route(
            "/proposals/{id}/revisions",
            post(create_revision).get(list_revisions),
        )
        .with_state(state)
}

/// `POST /proposals` — create a proposal directed at a mandate.
///
/// Creating a civic artifact is a citizen action, so the verified caller (from
/// [`dsoc_app::CallerId`], never the body — ADR-0007) must be at least email-verified within its org
/// before any write. Anonymous callers get `403 Forbidden`.
async fn create(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Json(req): Json<CreateProposalRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<ProposalDto>(&e);
    }
    let new = match NewProposal::validate(&req.title, &req.body, req.threshold) {
        Ok(n) => n,
        Err(e) => return error_response::<ProposalDto>(&e),
    };
    // Multi-destinatário (0537): principal primeiro, co-destinatários deduplicados e limitados.
    let targets =
        match crate::domain::normalize_targets(req.mandate_id, &req.additional_mandate_ids) {
            Ok(t) => t,
            Err(e) => return error_response::<ProposalDto>(&e),
        };
    let targets: Vec<MandateId> = targets.into_iter().map(MandateId::from_uuid).collect();
    let svc = ProposalService::from_state(&state);
    match svc
        .create_targets(caller.org, &targets, Some(caller.citizen), &new)
        .await
    {
        Ok(row) => {
            // Best-effort: o detalhe re-busca; uma falha aqui não pode derrubar o 201.
            let targets = svc
                .targets(ProposalId::from_uuid(row.id))
                .await
                .unwrap_or_default();
            (
                StatusCode::CREATED,
                Json(ApiResponse::ok(proposal_dto(row, targets))),
            )
                .into_response()
        }
        Err(e) => error_response::<ProposalDto>(&e),
    }
}

/// `GET /proposals/{id}` — fetch one proposal.
async fn get_one(State(state): State<dsoc_app::AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = ProposalService::from_state(&state);
    match svc.get(ProposalId::from_uuid(id)).await {
        Ok(row) => {
            let targets = svc
                .targets(ProposalId::from_uuid(row.id))
                .await
                .unwrap_or_default();
            (
                StatusCode::OK,
                Json(ApiResponse::ok(proposal_dto(row, targets))),
            )
                .into_response()
        }
        Err(e) => error_response::<ProposalDto>(&e),
    }
}

/// `GET /proposals?org_id=&after=&limit=` — keyset-paginated proposal list.
async fn list(
    State(state): State<dsoc_app::AppState>,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = ProposalService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let after = params.after.map(ProposalId::from_uuid);
    let limit = params.limit.unwrap_or(20);
    match svc.list(org, after, limit).await {
        Ok((rows, total)) => {
            // Listagem leve: targets só no detalhe/create (evita N+1 por página).
            let dtos: Vec<ProposalDto> = rows
                .into_iter()
                .map(|row| proposal_dto(row, Vec::new()))
                .collect();
            let meta = PageMeta {
                total: u64::try_from(total).unwrap_or_default(),
                limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(20),
                offset: 0,
            };
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<ProposalDto>>(&e),
    }
}

/// `POST /proposals/{id}/revisions` — append a revision (edit). Append-only; preserves history.
///
/// Editing is a citizen action; the verified caller (from [`dsoc_app::CallerId`], never the body —
/// ADR-0007) is authorized (email-verified) before any write and recorded as the revision's author.
async fn create_revision(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateRevisionRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<RevisionDto>(&e);
    }
    let new = match NewRevision::validate(&req.title, &req.body) {
        Ok(n) => n,
        Err(e) => return error_response::<RevisionDto>(&e),
    };
    let svc = ProposalService::from_state(&state);
    match svc
        .add_revision(ProposalId::from_uuid(id), caller.citizen, &new)
        .await
    {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(RevisionDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<RevisionDto>(&e),
    }
}

/// `GET /proposals/{id}/revisions?after=&limit=` — keyset-paginated revision history (oldest-first).
async fn list_revisions(
    State(state): State<dsoc_app::AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<RevisionListParams>,
) -> Response {
    let svc = ProposalService::from_state(&state);
    let limit = params.limit.unwrap_or(20);
    match svc
        .list_revisions(ProposalId::from_uuid(id), params.after, limit)
        .await
    {
        Ok(rows) => {
            let dtos: Vec<RevisionDto> = rows.into_iter().map(RevisionDto::from).collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(e) => error_response::<Vec<RevisionDto>>(&e),
    }
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no internals.
fn error_response<T: Serialize>(err: &Error) -> Response {
    let status = status_for(err);
    let body: ApiResponse<T> = ApiResponse::fail(err.code(), err.to_string());
    (status, Json(body)).into_response()
}

/// Map the canonical error model onto HTTP status codes (mirrors the gateway's contract).
fn status_for(err: &Error) -> StatusCode {
    match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_dto_maps_fields_and_clamps_negative_support() {
        let row = ProposalRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            cluster_id: Some(Uuid::now_v7()),
            title: "t".to_string(),
            body: "b".to_string(),
            status: "clustered".to_string(),
            support_count: -3,
            threshold: 100,
            threshold_crossed_at: None,
            published_at: None,
            author_citizen_id: None,
            author_handle: None,
            author_avatar_object_key: None,
            urgencia: "comum".to_owned(),
            notified_author_at: None,
            notified_mandate_at: None,
            created_at: Utc::now(),
        };
        let dto = proposal_dto(row.clone(), Vec::new());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.mandate_id, row.mandate_id);
        assert_eq!(dto.cluster_id, row.cluster_id);
        assert_eq!(
            dto.support_count, 0,
            "negative tally clamps to zero on the wire"
        );
    }

    #[test]
    fn revision_dto_from_row_round_trips() {
        let row = RevisionRow {
            id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            title: "novo".to_string(),
            body: "corpo".to_string(),
            edited_by: Uuid::now_v7(),
            created_at: Utc::now(),
        };
        let dto = RevisionDto::from(row.clone());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.edited_by, row.edited_by);
    }

    #[test]
    fn status_codes_match_error_kinds() {
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&Error::Storage(Box::new(std::io::Error::other("x")))),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
