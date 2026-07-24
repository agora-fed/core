//! Axum surface for `debates`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004).
//!
//! **Authorization (SECURITY.md / ADR-0007).** Every *mutating* handler authorizes the
//! authenticated caller through the verified [`dsoc_app::CallerId`] extractor — the acting
//! citizen and org are taken from the request's identity, NEVER from the body — and requires
//! at least an email-confirmed citizen via `authz.require` before any write. Reads are public.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{NewContribution, NewDebate};
use crate::queries::{ContributionRow, DebateRow};
use crate::service::{DebateService, DEFAULT_PAGE_LIMIT};

/// Minimum assurance required to open a debate or contribute: an email-confirmed citizen.
const REQUIRED_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/debates", post(create_debate).get(list_debates))
        .route("/debates/{id}", get(get_debate))
        .route(
            "/debates/{id}/contributions",
            post(contribute).get(list_contributions),
        )
        .with_state(state)
}

// --- DTOs ----------------------------------------------------------------------------

/// Public view of a debate.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DebateDto {
    /// Debate id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title — the motion under debate.
    pub title: String,
    /// Framing — the neutral context.
    pub framing: String,
    /// Optional UF territorial scope (`None` = nacional).
    pub uf: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<DebateRow> for DebateDto {
    fn from(row: DebateRow) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            framing: row.framing,
            uf: row.uf,
            created_at: row.created_at,
        }
    }
}

/// Public view of a contribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContributionDto {
    /// Contribution id.
    pub id: Uuid,
    /// The debate this contribution belongs to.
    pub debate_id: Uuid,
    /// Author (a citizen).
    pub author_id: Uuid,
    /// Stance taken (`pro` | `con` | `neutral`).
    pub stance: String,
    /// The contribution text.
    pub body: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<ContributionRow> for ContributionDto {
    fn from(row: ContributionRow) -> Self {
        Self {
            id: row.id,
            debate_id: row.debate_id,
            author_id: row.author_id,
            stance: row.stance,
            body: row.body,
            created_at: row.created_at,
        }
    }
}

/// Request to open a debate. The acting org comes from the verified [`dsoc_app::CallerId`]
/// (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateDebateRequest {
    /// Title — the motion/question under debate.
    pub title: String,
    /// Framing — the neutral context that frames the discussion.
    pub framing: String,
    /// Optional UF territorial scope (2-letter code; omit/blank = nacional).
    #[serde(default)]
    pub uf: Option<String>,
}

/// Request to contribute to a debate. The author and org come from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ContributeRequest {
    /// Stance taken (`pro` | `con` | `neutral`).
    pub stance: String,
    /// The contribution text.
    pub body: String,
}

/// Query parameters for listing debates (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return debates with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

/// Query parameters for listing a debate's contributions.
#[derive(Debug, Clone, Deserialize)]
pub struct ContributionListParams {
    /// Keyset cursor over ascending contribution id. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

// --- handlers ------------------------------------------------------------------------

/// `POST /debates` — open a debate owned by the caller's org.
///
/// Opening a debate is a citizen action, so the verified caller (from [`dsoc_app::CallerId`],
/// never the body — ADR-0007) must be at least email-verified within its org before any write.
async fn create_debate(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateDebateRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<DebateDto>(&e);
    }
    let new = match NewDebate::validate(&req.title, &req.framing, req.uf.as_deref()) {
        Ok(n) => n,
        Err(e) => return error_response::<DebateDto>(&e),
    };
    let svc = DebateService::from_state(&state);
    match svc.create_debate(caller.org, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(DebateDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<DebateDto>(&e),
    }
}

/// `GET /debates/{id}` — fetch one debate (public read).
async fn get_debate(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = DebateService::from_state(&state);
    match svc.get_debate(id).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(DebateDto::from(row)))).into_response(),
        Err(e) => error_response::<DebateDto>(&e),
    }
}

/// `GET /debates?org_id=&after=&limit=` — keyset-paginated debate list (public read).
async fn list_debates(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    let svc = DebateService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_debates(org, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<DebateDto> = rows.into_iter().map(DebateDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<DebateDto>>(&e),
    }
}

/// `POST /debates/{id}/contributions` — contribute a pro/con/neutral position to a debate.
///
/// Contributing is a citizen action; the verified caller (from [`dsoc_app::CallerId`], never the
/// body — ADR-0007) is authorized (email-verified) before any write and recorded as the author.
async fn contribute(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<ContributeRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<ContributionDto>(&e);
    }
    let new = match NewContribution::validate(&req.stance, &req.body) {
        Ok(n) => n,
        Err(e) => return error_response::<ContributionDto>(&e),
    };
    let svc = DebateService::from_state(&state);
    match svc.contribute(id, caller.citizen, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(ContributionDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<ContributionDto>(&e),
    }
}

/// `GET /debates/{id}/contributions?after=&limit=` — keyset-paginated contributions (public read).
async fn list_contributions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ContributionListParams>,
) -> Response {
    let svc = DebateService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_contributions(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<ContributionDto> = rows.into_iter().map(ContributionDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<ContributionDto>>(&e),
    }
}

// --- helpers -------------------------------------------------------------------------

/// Build pagination metadata, clamping the reported page size into the wire range.
fn page_meta(total: i64, limit: i64) -> PageMeta {
    PageMeta {
        total: u64::try_from(total).unwrap_or_default(),
        limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(DEFAULT_PAGE_LIMIT as u32),
        offset: 0,
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
        Error::Dependency { .. } => StatusCode::BAD_GATEWAY,
        // `Error` is #[non_exhaustive]; Storage and any future variant are server-side.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn debate_dto_maps_fields() {
        let row = DebateRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "Tarifa zero?".to_string(),
            framing: "Contexto".to_string(),
            uf: Some("SP".to_string()),
            created_at: at(),
        };
        let dto = DebateDto::from(row.clone());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.title, "Tarifa zero?");
        assert_eq!(dto.framing, "Contexto");
        assert_eq!(dto.uf.as_deref(), Some("SP"));
    }

    #[test]
    fn contribution_dto_carries_stance_token() {
        let row = ContributionRow {
            id: Uuid::now_v7(),
            debate_id: Uuid::now_v7(),
            author_id: Uuid::now_v7(),
            stance: "con".to_string(),
            body: "discordo".to_string(),
            created_at: at(),
        };
        let dto = ContributionDto::from(row.clone());
        assert_eq!(dto.stance, "con");
        assert_eq!(dto.author_id, row.author_id);
    }

    #[test]
    fn page_meta_clamps_and_counts() {
        let meta = page_meta(7, 20);
        assert_eq!(meta.total, 7);
        assert_eq!(meta.limit, 20);
        assert_eq!(meta.offset, 0);
        // A negative total clamps to zero; a zero limit clamps up to one.
        let degenerate = page_meta(-1, 0);
        assert_eq!(degenerate.total, 0);
        assert_eq!(degenerate.limit, 1);
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
