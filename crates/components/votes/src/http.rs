//! Axum HTTP surface. Exposes `pub fn routes(state: AppState) -> Router<()>` (ADR-0004 wiring);
//! it never binds a socket — the gateway owns the IPv6 bind. Domain results are mapped to the
//! crate's DTOs wrapped in the uniform `api-contract` `ApiResponse` envelope, and
//! `dsoc_core::Error` is mapped to HTTP status without leaking internal detail (SECURITY.md).
//!
//! ## Authorization (mutations)
//! [`cast`] is the only mutating handler. Per ADR-0007, the acting citizen identity comes from
//! the authenticated `CallerId` extractor (cookie middleware), **never** from the request body —
//! trusting a body-supplied `citizen_id` would let any logged-in user impersonate another. After
//! the extractor proves who the caller is, `Arc<dyn Authorization>::require` then asserts the
//! caller meets the minimum verification level (anonymous ⇒ Forbidden).

use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::ProposalId;
use dsoc_core::Error;

use crate::domain::MIN_VOTE_LEVEL;
use crate::dto::{CastVoteRequest, TallyDto, VoteReceiptDto};
use crate::service::VoteService;

/// Build the routed service surface mounted by the gateway. Takes the frozen [`AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/votes", post(cast))
        .route("/votes/tallies", get(list_tallies))
        .route("/votes/tally/{proposal_id}", get(get_tally))
        .with_state(state)
}

/// Query parameters for the keyset-paginated aggregate listing.
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Keyset cursor: return tallies with a proposal id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// `POST /votes` — cast a support signal. Identity comes from the authenticated `CallerId`
/// (cookie middleware) — the request body's `citizen_id`/`org_id` are ignored to close the
/// "trust the body" impersonation hole (ADR-0007). Returns `201 Created` with the receipt.
pub async fn cast(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CastVoteRequest>,
) -> Response {
    // Authorize the authenticated caller BEFORE any write. Anonymous/unknown ⇒ Forbidden.
    if let Err(e) = state.authz.require(caller.org, caller.citizen, MIN_VOTE_LEVEL).await {
        return error_response::<VoteReceiptDto>(&e);
    }

    let proposal = ProposalId::from_uuid(req.proposal_id);
    let svc = VoteService::from_state(&state);
    match svc.cast(caller.org, proposal, caller.citizen).await {
        Ok(receipt) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(VoteReceiptDto::from(receipt))),
        )
            .into_response(),
        Err(e) => error_response::<VoteReceiptDto>(&e),
    }
}

/// `GET /votes/tally/{proposal_id}` — read the privacy-safe aggregate for one proposal
/// (official-facing). No authorization gate: the aggregate is public and carries no linkage.
pub async fn get_tally(State(state): State<AppState>, Path(proposal_id): Path<Uuid>) -> Response {
    let svc = VoteService::from_state(&state);
    match svc.tally(ProposalId::from_uuid(proposal_id)).await {
        Ok(view) => (StatusCode::OK, Json(ApiResponse::ok(TallyDto::from(view)))).into_response(),
        Err(e) => error_response::<TallyDto>(&e),
    }
}

/// `GET /votes/tallies?after=&limit=` — keyset-paginated aggregate browse (official-facing).
pub async fn list_tallies(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = VoteService::from_state(&state);
    let after = params.after.map(ProposalId::from_uuid);
    let limit = params.limit.unwrap_or(20);
    match svc.list_tallies(after, limit).await {
        Ok((views, total)) => {
            let dtos: Vec<TallyDto> = views.into_iter().map(TallyDto::from).collect();
            let meta = PageMeta {
                total: u64::try_from(total).unwrap_or_default(),
                limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(20),
                offset: 0,
            };
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<TallyDto>>(&e),
    }
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no internals.
fn error_response<T: Serialize>(err: &Error) -> Response {
    if matches!(err, Error::Storage(_) | Error::Dependency { .. }) {
        tracing::error!(code = err.code(), detail = %err, "votes request failed");
    }
    let body: ApiResponse<T> = ApiResponse::fail(err.code(), message_for(err));
    (status_for(err), Json(body)).into_response()
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
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// End-user message (Portuguese — civic content policy) that never leaks internal detail.
fn message_for(err: &Error) -> &'static str {
    match err {
        Error::NotFound(_) => "Recurso não encontrado.",
        Error::Forbidden(_) => "Acesso negado.",
        Error::Unauthorized => "Não autenticado.",
        Error::Validation(_) => "Dados inválidos.",
        Error::Conflict(_) => "Você já apoiou esta proposta.",
        Error::Dependency { .. } => "Falha ao contatar dependência soberana.",
        _ => "Erro interno do servidor.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map_each_error() {
        assert_eq!(
            status_for(&Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_for(&Error::Storage("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn messages_are_safe_and_nonempty() {
        // The storage message must not echo internal detail.
        assert!(!message_for(&Error::Storage("secret-dsn".into())).contains("secret-dsn"));
        assert!(!message_for(&Error::Forbidden("x".into())).is_empty());
        assert!(!message_for(&Error::Conflict("x".into())).is_empty());
    }
}
