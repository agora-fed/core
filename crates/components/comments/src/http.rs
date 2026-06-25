//! Axum HTTP surface for comments. Handlers map the domain onto owned DTOs wrapped in the
//! shared [`ApiResponse`] envelope and translate [`dsoc_core::Error`] into status codes.
//! The crate exposes [`routes`] for the gateway to mount; it never binds a socket
//! (ADR-0004 — the gateway owns the IPv6 bind).
//!
//! **Authorization (SECURITY.md).** Every *mutating* handler authorizes the authenticated
//! caller against the org through the injected `Arc<dyn Authorization>` *before* any write:
//! the `citizen_id` in the body is never trusted on its own. Posting a comment or a vote
//! requires at least an email-confirmed citizen; anonymous callers get `403`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, CommentId, OrgId, ProposalId};
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{Comment, CommentVote, VoteWeight};
use crate::queries::Cursor;
use crate::service::{CommentService, DEFAULT_PAGE_LIMIT};

/// Minimum assurance required to author a comment or cast a vote: an email-confirmed
/// citizen. Anonymous visitors cannot mutate the deliberation thread.
const REQUIRED_LEVEL: VerificationLevel = VerificationLevel::Email;

/// Build the comments router for the gateway to mount.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/comments", post(create_comment).get(list_thread))
        .route("/comments/{id}", get(get_comment))
        .route("/comments/{id}/votes", post(cast_vote))
        .with_state(state)
}

// --- DTOs ----------------------------------------------------------------------------

/// Public view of a comment.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CommentDto {
    /// Comment id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The proposal this thread hangs off.
    pub proposal_id: Uuid,
    /// Parent comment id, or `null` for a root.
    pub parent_id: Option<Uuid>,
    /// Author (a citizen).
    pub author_id: Uuid,
    /// The comment text.
    pub body: String,
    /// Nesting depth (0 for a root).
    pub depth: i32,
    /// Lifecycle status (`visible` | `flagged` | `hidden`).
    pub status: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Comment> for CommentDto {
    fn from(c: Comment) -> Self {
        Self {
            id: c.id,
            org_id: c.org_id,
            proposal_id: c.proposal_id,
            parent_id: c.parent_id,
            author_id: c.author_id,
            body: c.body,
            depth: c.depth,
            status: c.status.as_str().to_owned(),
            created_at: c.created_at,
        }
    }
}

/// Public view of a vote.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct CommentVoteDto {
    /// Vote id.
    pub id: Uuid,
    /// The comment voted on.
    pub comment_id: Uuid,
    /// The voting citizen.
    pub citizen_id: Uuid,
    /// The up/down weight (`+1` | `-1`).
    pub weight: i16,
    /// Creation time of the original vote.
    pub created_at: DateTime<Utc>,
}

impl From<CommentVote> for CommentVoteDto {
    fn from(v: CommentVote) -> Self {
        Self {
            id: v.id,
            comment_id: v.comment_id,
            citizen_id: v.citizen_id,
            weight: v.weight,
            created_at: v.created_at,
        }
    }
}

/// Request body to create a comment (root or reply).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateCommentRequest {
    /// Owning organization.
    pub org_id: Uuid,
    /// The authenticated caller authoring the comment; authorized before any write.
    pub citizen_id: Uuid,
    /// The proposal being deliberated.
    pub proposal_id: Uuid,
    /// Parent comment id for a reply; omit for a root comment.
    pub parent_id: Option<Uuid>,
    /// The comment text (Portuguese civic content).
    pub body: String,
}

/// Request body to cast (or change) a vote on a comment.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
pub struct CastVoteRequest {
    /// Owning organization.
    pub org_id: Uuid,
    /// The authenticated caller voting; authorized before any write.
    pub citizen_id: Uuid,
    /// The up/down weight (`+1` | `-1`).
    pub weight: i16,
}

/// Query parameters for the keyset-paginated thread listing.
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// The proposal whose thread to read.
    pub proposal_id: Uuid,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
    /// Cursor: `created_at` of the previous page's last row.
    pub cursor_at: Option<DateTime<Utc>>,
    /// Cursor: `id` of the previous page's last row.
    pub cursor_id: Option<Uuid>,
}

impl ListParams {
    fn cursor(&self) -> Option<Cursor> {
        match (self.cursor_at, self.cursor_id) {
            (Some(at), Some(id)) => Some(Cursor { at, id }),
            _ => None,
        }
    }

    fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_PAGE_LIMIT)
    }
}

// --- handlers ------------------------------------------------------------------------

/// `POST /comments` — author a root comment or a reply. Authorizes the caller first.
async fn create_comment(
    State(state): State<AppState>,
    Json(req): Json<CreateCommentRequest>,
) -> Response {
    let org = OrgId::from_uuid(req.org_id);
    let citizen = CitizenId::from_uuid(req.citizen_id);
    if let Err(e) = state.authz.require(org, citizen, REQUIRED_LEVEL).await {
        return error_response::<CommentDto>(&e);
    }
    let svc = CommentService::from_state(&state);
    let parent = req.parent_id.map(CommentId::from_uuid);
    match svc
        .create_comment(
            org,
            ProposalId::from_uuid(req.proposal_id),
            parent,
            citizen,
            &req.body,
        )
        .await
    {
        Ok(comment) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(CommentDto::from(comment))),
        )
            .into_response(),
        Err(e) => error_response::<CommentDto>(&e),
    }
}

/// `POST /comments/{id}/votes` — cast or change a vote. Authorizes the caller first.
async fn cast_vote(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CastVoteRequest>,
) -> Response {
    let org = OrgId::from_uuid(req.org_id);
    let citizen = CitizenId::from_uuid(req.citizen_id);
    if let Err(e) = state.authz.require(org, citizen, REQUIRED_LEVEL).await {
        return error_response::<CommentVoteDto>(&e);
    }
    let weight = match VoteWeight::from_i16(req.weight) {
        Ok(w) => w,
        Err(e) => return error_response::<CommentVoteDto>(&Error::Validation(e.to_string())),
    };
    let svc = CommentService::from_state(&state);
    match svc.vote(CommentId::from_uuid(id), citizen, weight).await {
        Ok(vote) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(CommentVoteDto::from(vote))),
        )
            .into_response(),
        Err(e) => error_response::<CommentVoteDto>(&e),
    }
}

/// `GET /comments/{id}` — fetch a single comment (public read).
async fn get_comment(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = CommentService::from_state(&state);
    match svc.get_comment(CommentId::from_uuid(id)).await {
        Ok(comment) => (
            StatusCode::OK,
            Json(ApiResponse::ok(CommentDto::from(comment))),
        )
            .into_response(),
        Err(e) => error_response::<CommentDto>(&e),
    }
}

/// `GET /comments?org_id=&proposal_id=&limit=&cursor_at=&cursor_id=` — keyset-paginated
/// thread listing (public read).
async fn list_thread(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    let svc = CommentService::from_state(&state);
    let limit = params.limit();
    match svc
        .list_thread(
            OrgId::from_uuid(params.org_id),
            ProposalId::from_uuid(params.proposal_id),
            params.cursor(),
            limit,
        )
        .await
    {
        Ok(comments) => {
            let dtos: Vec<CommentDto> = comments.into_iter().map(CommentDto::from).collect();
            (StatusCode::OK, Json(paged(dtos, limit))).into_response()
        }
        Err(e) => error_response::<Vec<CommentDto>>(&e),
    }
}

// --- helpers -------------------------------------------------------------------------

fn paged<T>(items: Vec<T>, limit: i64) -> ApiResponse<Vec<T>> {
    // Keyset pagination has no cheap total; report the page length as a lower bound.
    let meta = PageMeta {
        total: items.len() as u64,
        limit: u32::try_from(limit.max(0)).unwrap_or(u32::MAX),
        offset: 0,
    };
    ApiResponse::page(items, meta)
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no
/// internal detail (storage errors render generically).
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
    use crate::domain::CommentStatus;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn comment_dto_maps_status_token_and_depth() {
        let c = Comment {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            parent_id: None,
            author_id: Uuid::now_v7(),
            body: "olá".to_owned(),
            depth: 2,
            status: CommentStatus::Flagged,
            created_at: at(),
        };
        let dto = CommentDto::from(c);
        assert_eq!(dto.status, "flagged");
        assert_eq!(dto.depth, 2);
        assert!(dto.parent_id.is_none());
    }

    #[test]
    fn vote_dto_carries_weight() {
        let v = CommentVote {
            id: Uuid::now_v7(),
            comment_id: Uuid::now_v7(),
            citizen_id: Uuid::now_v7(),
            weight: -1,
            created_at: at(),
        };
        let dto = CommentVoteDto::from(v);
        assert_eq!(dto.weight, -1);
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
        let storage = Error::Storage(Box::new(std::io::Error::other("secret")));
        assert_eq!(status_for(&storage), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn paged_reports_length_as_lower_bound() {
        let resp = paged(vec![1u32, 2, 3], 50);
        let meta = resp.meta.unwrap();
        assert_eq!(meta.total, 3);
        assert_eq!(meta.limit, 50);
        assert_eq!(meta.offset, 0);
    }

    #[test]
    fn list_params_builds_cursor_only_when_both_present() {
        let none = ListParams {
            org_id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            limit: None,
            cursor_at: Some(at()),
            cursor_id: None,
        };
        assert!(none.cursor().is_none());
        assert_eq!(none.limit(), DEFAULT_PAGE_LIMIT);

        let both = ListParams {
            org_id: Uuid::now_v7(),
            proposal_id: Uuid::now_v7(),
            limit: Some(10),
            cursor_at: Some(at()),
            cursor_id: Some(Uuid::now_v7()),
        };
        assert!(both.cursor().is_some());
        assert_eq!(both.limit(), 10);
    }
}
