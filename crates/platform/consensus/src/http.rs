//! Axum surface for `consensus`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004). The crate owns its
//! request/response DTOs and their `utoipa` schema fragments here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_core::ids::{CitizenId, ClusterId, OrgId, ProposalId};
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::Placement;
use crate::queries::ClusterRow;
use crate::service::ClusterService;

/// Request to ingest (embed + cluster) a proposal. The body is supplied here because the slim
/// `proposals.created` event carries ids only.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct IngestRequest {
    /// Owning organization.
    pub org_id: Uuid,
    /// The authenticated caller triggering the ingest. Clustering mutates civic infrastructure, so
    /// the caller is authorized against the org before any write (see [`ingest`]).
    pub citizen_id: Uuid,
    /// The proposal to cluster.
    pub proposal_id: Uuid,
    /// The proposal text to embed (Portuguese civic content).
    pub text: String,
}

/// The outcome of ingesting a proposal.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct ClusterOutcomeDto {
    /// `true` if the proposal merged into an existing cluster; `false` if it seeded a new one.
    pub merged: bool,
    /// The cluster the proposal now belongs to.
    pub cluster_id: Uuid,
    /// The cluster's live member count after the operation.
    pub size: u32,
}

/// Public view of a consensus cluster.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct ClusterDto {
    /// Cluster id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Live member count.
    pub size: u32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<ClusterRow> for ClusterDto {
    fn from(row: ClusterRow) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            size: u32::try_from(row.size).unwrap_or_default(),
            created_at: row.created_at,
        }
    }
}

/// Query parameters for listing clusters (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return clusters with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: dsoc_app::AppState) -> Router<()> {
    Router::new()
        .route("/consensus/proposals", post(ingest))
        .route("/consensus/clusters", get(list_clusters))
        .route("/consensus/clusters/{id}", get(get_cluster))
        .with_state(state)
}

/// `POST /consensus/proposals` — embed and cluster a proposal.
///
/// Triggering clustering is an administrative action on politically-sensitive civic infrastructure,
/// so the authenticated caller (`citizen_id`) must be at least directory-verified within the org
/// (`Authorization::require`) before any write. Anonymous/email-only callers get `403 Forbidden`.
async fn ingest(
    State(state): State<dsoc_app::AppState>,
    Json(req): Json<IngestRequest>,
) -> Response {
    let org = OrgId::from_uuid(req.org_id);
    let citizen = CitizenId::from_uuid(req.citizen_id);
    if let Err(e) = state
        .authz
        .require(org, citizen, VerificationLevel::Directory)
        .await
    {
        return error_response::<ClusterOutcomeDto>(&e);
    }
    let svc = ClusterService::from_state(&state);
    let proposal = ProposalId::from_uuid(req.proposal_id);
    match svc.ingest(org, proposal, &req.text).await {
        Ok(placement) => (
            StatusCode::OK,
            Json(ApiResponse::ok(outcome_dto(placement))),
        )
            .into_response(),
        Err(e) => error_response::<ClusterOutcomeDto>(&e),
    }
}

/// `GET /consensus/clusters/{id}` — fetch one cluster.
async fn get_cluster(State(state): State<dsoc_app::AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = ClusterService::from_state(&state);
    match svc.cluster(ClusterId::from_uuid(id)).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(ClusterDto::from(row)))).into_response(),
        Err(e) => error_response::<ClusterDto>(&e),
    }
}

/// `GET /consensus/clusters?org_id=&after=&limit=` — keyset-paginated cluster list.
async fn list_clusters(
    State(state): State<dsoc_app::AppState>,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = ClusterService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let after = params.after.map(ClusterId::from_uuid);
    let limit = params.limit.unwrap_or(20);
    match svc.list_clusters(org, after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<ClusterDto> = rows.into_iter().map(ClusterDto::from).collect();
            let meta = PageMeta {
                total: u64::try_from(total).unwrap_or_default(),
                limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(20),
                offset: 0,
            };
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<ClusterDto>>(&e),
    }
}

/// Map a [`Placement`] to the wire DTO.
fn outcome_dto(placement: Placement) -> ClusterOutcomeDto {
    match placement {
        Placement::Merged { cluster, size } => ClusterOutcomeDto {
            merged: true,
            cluster_id: cluster.as_uuid(),
            size,
        },
        Placement::Formed { cluster, .. } => ClusterOutcomeDto {
            merged: false,
            cluster_id: cluster.as_uuid(),
            size: 1,
        },
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
    fn outcome_dto_marks_merge_and_form() {
        let cluster = ClusterId::new();
        let merged = outcome_dto(Placement::Merged { cluster, size: 4 });
        assert!(merged.merged && merged.size == 4 && merged.cluster_id == cluster.as_uuid());

        let formed = outcome_dto(Placement::Formed {
            cluster,
            proposal: ProposalId::new(),
        });
        assert!(!formed.merged && formed.size == 1);
    }

    #[test]
    fn status_codes_match_error_kinds() {
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
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

    #[test]
    fn cluster_dto_from_row_clamps_negative_size() {
        let row = ClusterRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            size: -1,
            created_at: Utc::now(),
        };
        assert_eq!(ClusterDto::from(row).size, 0);
    }
}
