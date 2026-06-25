//! Axum surface for `consequence`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004). The crate owns its
//! request/response DTOs and their `utoipa` schema fragments here.
//!
//! Every MUTATING handler AUTHORIZES the authenticated caller against the org through the injected
//! `Arc<dyn Authorization>` BEFORE any write. The acting identity is taken from the verified
//! `dsoc_app::CallerId` extractor (ADR-0007) — never from the request body — closing the
//! "trust citizen_id from the body" authorization bypass. Recording an official response and sweeping
//! SLAs into public silence are accountability-critical actions, so both require the caller to be at
//! least directory-verified. A mandate is never trusted from a request body — the response handler
//! uses the SLA's own stored `mandate_id`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta, SlaStatus};
use dsoc_core::ids::{OrgId, SlaId};
use dsoc_core::{Error, Result, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain;
use crate::queries::SlaRow;
use crate::service::{ConsequenceService, RespondOutcome};

/// Request for an official to respond to an SLA. The acting official and org come from the verified
/// `dsoc_app::CallerId` extractor (ADR-0007), never from this body; the responding mandate is taken
/// from the SLA, not from the request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RespondRequest {
    /// The public response text (Portuguese civic content).
    pub body: String,
    /// `true` if the response carries a concrete commitment to act (→ `Acted`), else `Answered`.
    pub committed: bool,
}

/// The outcome of recording an official response.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct RespondOutcomeDto {
    /// The SLA that was resolved.
    pub sla_id: Uuid,
    /// The new terminal status.
    pub status: SlaStatus,
}

impl From<RespondOutcome> for RespondOutcomeDto {
    fn from(outcome: RespondOutcome) -> Self {
        Self {
            sla_id: outcome.sla_id.as_uuid(),
            status: outcome.status,
        }
    }
}

/// The outcome of an expiry sweep.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct SweepOutcomeDto {
    /// How many SLAs expired into public silence on this pass.
    pub expired: u64,
}

/// Public view of an SLA clock (the emotional core of the UI — PLAN.md section 8).
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct SlaDto {
    /// SLA id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The targeted official's mandate.
    pub mandate_id: Uuid,
    /// The consensus cluster the demand belongs to.
    pub cluster_id: Uuid,
    /// The proposal that crossed the threshold.
    pub proposal_id: Uuid,
    /// Current status.
    pub status: SlaStatus,
    /// When the clock started.
    pub started_at: DateTime<Utc>,
    /// The response deadline.
    pub due_at: DateTime<Utc>,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
}

impl TryFrom<SlaRow> for SlaDto {
    type Error = Error;

    fn try_from(row: SlaRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            org_id: row.org_id,
            mandate_id: row.mandate_id,
            cluster_id: row.cluster_id,
            proposal_id: row.proposal_id,
            status: domain::status_from_str(&row.status)?,
            started_at: row.started_at,
            due_at: row.due_at,
            created_at: row.created_at,
        })
    }
}

/// Query parameters for listing SLAs (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return SLAs with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: dsoc_app::AppState) -> Router<()> {
    Router::new()
        .route("/consequence/slas", get(list_slas))
        .route("/consequence/slas/{id}", get(get_sla))
        .route("/consequence/slas/{id}/responses", post(respond))
        .route("/consequence/sweep", post(sweep))
        .with_state(state)
}

/// `POST /consequence/slas/{id}/responses` — an official records a public response.
///
/// The acting official and org come from the verified `dsoc_app::CallerId` extractor (ADR-0007),
/// never from the request body. The caller must be at least directory-verified within the org before
/// any write (`Authorization::require`); anonymous/email-only callers get `403 Forbidden`. The
/// responding mandate is the SLA's own `mandate_id`, never trusted from the request body.
async fn respond(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<RespondRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Directory)
        .await
    {
        return error_response::<RespondOutcomeDto>(&e);
    }
    let svc = ConsequenceService::from_state(&state);
    match svc
        .respond(SlaId::from_uuid(id), &req.body, req.committed)
        .await
    {
        Ok(outcome) => (
            StatusCode::OK,
            Json(ApiResponse::ok(RespondOutcomeDto::from(outcome))),
        )
            .into_response(),
        Err(e) => error_response::<RespondOutcomeDto>(&e),
    }
}

/// `POST /consequence/sweep` — record public silence for every expired SLA in the caller's org.
/// Administrative: the acting service/operator identity comes from the verified `dsoc_app::CallerId`
/// extractor (ADR-0007), never from a request body, and is authorized at directory level within its
/// own org. The sweep is scoped to `caller.org`, so it never touches another tenant's clocks. Time
/// comes from the injected clock, never ambiently.
async fn sweep(State(state): State<dsoc_app::AppState>, caller: dsoc_app::CallerId) -> Response {
    let org = caller.org;
    if let Err(e) = state
        .authz
        .require(org, caller.citizen, VerificationLevel::Directory)
        .await
    {
        return error_response::<SweepOutcomeDto>(&e);
    }
    let now = state.clock.now();
    let svc = ConsequenceService::from_state(&state);
    match svc.sweep_expired(org, now).await {
        Ok(expired) => (
            StatusCode::OK,
            Json(ApiResponse::ok(SweepOutcomeDto { expired })),
        )
            .into_response(),
        Err(e) => error_response::<SweepOutcomeDto>(&e),
    }
}

/// `GET /consequence/slas/{id}` — fetch one SLA (public read).
async fn get_sla(State(state): State<dsoc_app::AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = ConsequenceService::from_state(&state);
    match svc.get_sla(SlaId::from_uuid(id)).await {
        Ok(row) => match SlaDto::try_from(row) {
            Ok(dto) => (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response(),
            Err(e) => error_response::<SlaDto>(&e),
        },
        Err(e) => error_response::<SlaDto>(&e),
    }
}

/// `GET /consequence/slas?org_id=&after=&limit=` — keyset-paginated SLA list (public read).
async fn list_slas(
    State(state): State<dsoc_app::AppState>,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = ConsequenceService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let after = params.after.map(SlaId::from_uuid);
    let limit = params.limit.unwrap_or(20);
    match svc.list_slas(org, after, limit).await {
        Ok((rows, total)) => {
            let dtos: Result<Vec<SlaDto>> = rows.into_iter().map(SlaDto::try_from).collect();
            match dtos {
                Ok(dtos) => {
                    let meta = PageMeta {
                        total: u64::try_from(total).unwrap_or_default(),
                        limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(20),
                        offset: 0,
                    };
                    (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
                }
                Err(e) => error_response::<Vec<SlaDto>>(&e),
            }
        }
        Err(e) => error_response::<Vec<SlaDto>>(&e),
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
    fn sla_dto_try_from_parses_status() {
        let row = SlaRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            cluster_id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            started_at: Utc::now(),
            due_at: Utc::now(),
            status: "ignored".to_string(),
            created_at: Utc::now(),
        };
        let dto = SlaDto::try_from(row).unwrap();
        assert_eq!(dto.status, SlaStatus::Ignored);
    }

    #[test]
    fn sla_dto_try_from_rejects_corrupt_status() {
        let row = SlaRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            cluster_id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            started_at: Utc::now(),
            due_at: Utc::now(),
            status: "garbage".to_string(),
            created_at: Utc::now(),
        };
        assert!(SlaDto::try_from(row).is_err());
    }

    #[test]
    fn respond_outcome_dto_carries_status() {
        let outcome = RespondOutcome {
            sla_id: SlaId::new(),
            status: SlaStatus::Acted,
        };
        let dto = RespondOutcomeDto::from(outcome);
        assert_eq!(dto.status, SlaStatus::Acted);
        assert_eq!(dto.sla_id, outcome.sla_id.as_uuid());
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
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Storage(Box::new(std::io::Error::other("x")))),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
