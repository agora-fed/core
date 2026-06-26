//! Axum surface for `surveys`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
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
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{NewQuestion, NewResponse, NewSurvey};
use crate::dto::{
    AddQuestionRequest, CreateSurveyRequest, QuestionDto, ResponseDto, SubmitResponseRequest,
    SurveyDto,
};
use crate::service::{SurveyService, DEFAULT_PAGE_LIMIT};

/// Minimum assurance required to author a survey, add a question, publish, or respond: an
/// email-confirmed citizen.
const REQUIRED_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/surveys", post(create_survey).get(list_surveys))
        .route("/surveys/{id}", get(get_survey))
        .route("/surveys/{id}/publish", post(publish_survey))
        .route(
            "/surveys/{id}/questions",
            post(add_question).get(list_questions),
        )
        .route(
            "/surveys/{id}/responses",
            post(submit_response).get(list_responses),
        )
        .with_state(state)
}

/// Query parameters for listing surveys (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListSurveysParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return surveys with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

/// Query parameters for listing a survey's child rows (questions/responses).
#[derive(Debug, Clone, Deserialize)]
pub struct ChildListParams {
    /// Keyset cursor over ascending child id. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

// --- handlers ------------------------------------------------------------------------

/// `POST /surveys` — open a draft survey owned by the caller's org.
///
/// Authoring is a citizen action, so the verified caller (from [`dsoc_app::CallerId`], never
/// the body — ADR-0007) must be at least email-verified within its org before any write.
async fn create_survey(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateSurveyRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<SurveyDto>(&e);
    }
    let new = match NewSurvey::validate(&req.title) {
        Ok(n) => n,
        Err(e) => return error_response::<SurveyDto>(&e),
    };
    let svc = SurveyService::from_state(&state);
    match svc.create_survey(caller.org, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(SurveyDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<SurveyDto>(&e),
    }
}

/// `GET /surveys/{id}` — fetch one survey (public read).
async fn get_survey(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = SurveyService::from_state(&state);
    match svc.get_survey(id).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(SurveyDto::from(row)))).into_response(),
        Err(e) => error_response::<SurveyDto>(&e),
    }
}

/// `GET /surveys?org_id=&after=&limit=` — keyset-paginated survey list (public read).
async fn list_surveys(
    State(state): State<AppState>,
    Query(params): Query<ListSurveysParams>,
) -> Response {
    let svc = SurveyService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_surveys(org, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<SurveyDto> = rows.into_iter().map(SurveyDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<SurveyDto>>(&e),
    }
}

/// `POST /surveys/{id}/publish` — publish a draft survey, opening it for responses.
///
/// Publishing is a citizen action; the verified caller (ADR-0007) is authorized before the
/// guarded state transition (draft -> published).
async fn publish_survey(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<SurveyDto>(&e);
    }
    let svc = SurveyService::from_state(&state);
    match svc.publish_survey(caller.org, id).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(SurveyDto::from(row)))).into_response(),
        Err(e) => error_response::<SurveyDto>(&e),
    }
}

/// `POST /surveys/{id}/questions` — add a typed question to a draft survey.
///
/// Editing the form is a citizen action; the verified caller (ADR-0007) is authorized before
/// any write.
async fn add_question(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<AddQuestionRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<QuestionDto>(&e);
    }
    let new = match NewQuestion::validate(&req.prompt, &req.kind, req.position) {
        Ok(n) => n,
        Err(e) => return error_response::<QuestionDto>(&e),
    };
    let svc = SurveyService::from_state(&state);
    match svc.add_question(caller.org, id, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(QuestionDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<QuestionDto>(&e),
    }
}

/// `GET /surveys/{id}/questions?after=&limit=` — keyset-paginated questions (public read).
async fn list_questions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ChildListParams>,
) -> Response {
    let svc = SurveyService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_questions(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<QuestionDto> = rows.into_iter().map(QuestionDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<QuestionDto>>(&e),
    }
}

/// `POST /surveys/{id}/responses` — submit a citizen's answers to a published survey.
///
/// Responding is a citizen action; the verified caller (from [`dsoc_app::CallerId`], never the
/// body — ADR-0007) is authorized (email-verified) before any write and recorded as the
/// respondent. One response per citizen: a re-submission idempotently returns the existing one.
async fn submit_response(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitResponseRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<ResponseDto>(&e);
    }
    let new = match NewResponse::validate(req.answers) {
        Ok(n) => n,
        Err(e) => return error_response::<ResponseDto>(&e),
    };
    let svc = SurveyService::from_state(&state);
    match svc.submit_response(id, caller.citizen, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(ResponseDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<ResponseDto>(&e),
    }
}

/// `GET /surveys/{id}/responses?after=&limit=` — keyset-paginated responses (public read).
async fn list_responses(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ChildListParams>,
) -> Response {
    let svc = SurveyService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_responses(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<ResponseDto> = rows.into_iter().map(ResponseDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<ResponseDto>>(&e),
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
