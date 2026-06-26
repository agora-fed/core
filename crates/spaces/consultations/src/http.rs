//! Axum HTTP surface. Exposes `pub fn routes(state: AppState) -> Router<()>` (ADR-0004 wiring); it
//! never binds a socket — the gateway owns the IPv6 bind. Domain results are mapped to the crate's
//! DTOs wrapped in the uniform `api-contract` `ApiResponse` envelope, and `dsoc_core::Error` is
//! mapped to HTTP status without leaking internal detail (SECURITY.md).
//!
//! ## Authorization (ADR-0007)
//! Both mutating handlers ([`create`], [`close`]) take the acting citizen from the
//! [`dsoc_app::CallerId`] extractor — never the body — and gate it through the injected
//! `Arc<dyn Authorization>` (`require(org, citizen, Directory)`) **before** any write. Read handlers
//! still take the caller for org scoping, so a tenant can only ever see its own consultations.

use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::SpaceId;
use dsoc_core::Error;

use crate::domain::MIN_MANAGE_LEVEL;
use crate::dto::{ConsultationDto, CreateConsultationRequest};
use crate::service::ConsultationService;

/// Build the routed service surface mounted by the gateway. Takes the frozen [`AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/consultations", post(create).get(list))
        .route("/consultations/{id}", get(get_one))
        .route("/consultations/{id}/close", post(close))
        .with_state(state)
}

/// Query parameters for the keyset-paginated consultation listing.
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Keyset cursor: return consultations with an id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side to 1..=100).
    pub limit: Option<i64>,
}

/// `POST /consultations` — create a consultation with its questions. Mutating: authorizes the
/// caller (directory-verified minimum) before any write. Returns `201 Created` with the stored
/// consultation. Org and acting citizen come from [`CallerId`], not the body (ADR-0007).
pub async fn create(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateConsultationRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, MIN_MANAGE_LEVEL)
        .await
    {
        return error_response::<ConsultationDto>(&e);
    }

    let svc = ConsultationService::from_state(&state);
    match svc
        .create(
            caller.org,
            &req.title,
            req.opens_at,
            req.closes_at,
            &req.questions,
        )
        .await
    {
        Ok((view, questions)) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(ConsultationDto::from_views(
                view, questions,
            ))),
        )
            .into_response(),
        Err(e) => error_response::<ConsultationDto>(&e),
    }
}

/// `POST /consultations/{id}/close` — close an open consultation. Mutating: authorizes the caller
/// (directory-verified minimum) before the guarded transition. Returns `200 OK` with the closed
/// consultation header.
pub async fn close(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, MIN_MANAGE_LEVEL)
        .await
    {
        return error_response::<ConsultationDto>(&e);
    }

    let svc = ConsultationService::from_state(&state);
    match svc.close(caller.org, SpaceId::from_uuid(id)).await {
        Ok(view) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ConsultationDto::from_views(
                view,
                Vec::new(),
            ))),
        )
            .into_response(),
        Err(e) => error_response::<ConsultationDto>(&e),
    }
}

/// `GET /consultations/{id}` — read one consultation and its questions, scoped to the caller's org.
pub async fn get_one(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    let svc = ConsultationService::from_state(&state);
    match svc.get(caller.org, SpaceId::from_uuid(id)).await {
        Ok((view, questions)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ConsultationDto::from_views(
                view, questions,
            ))),
        )
            .into_response(),
        Err(e) => error_response::<ConsultationDto>(&e),
    }
}

/// `GET /consultations?after=&limit=` — keyset-paginated browse of the caller's org consultations.
pub async fn list(
    State(state): State<AppState>,
    caller: CallerId,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = ConsultationService::from_state(&state);
    let after = params.after.map(SpaceId::from_uuid);
    let limit = params.limit.unwrap_or(20);
    match svc.list(caller.org, after, limit).await {
        Ok((views, total)) => {
            let dtos: Vec<ConsultationDto> = views
                .into_iter()
                .map(|v| ConsultationDto::from_views(v, Vec::new()))
                .collect();
            let meta = PageMeta {
                total: u64::try_from(total).unwrap_or_default(),
                limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(20),
                offset: 0,
            };
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<ConsultationDto>>(&e),
    }
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no internals.
fn error_response<T: Serialize>(err: &Error) -> Response {
    if matches!(err, Error::Storage(_) | Error::Dependency { .. }) {
        tracing::error!(code = err.code(), detail = %err, "consultations request failed");
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
        Error::NotFound(_) => "Consulta não encontrada.",
        Error::Forbidden(_) => "Acesso negado.",
        Error::Unauthorized => "Não autenticado.",
        Error::Validation(_) => "Dados inválidos.",
        Error::Conflict(_) => "A consulta já está encerrada.",
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
        assert!(!message_for(&Error::Storage("secret-dsn".into())).contains("secret-dsn"));
        assert!(!message_for(&Error::Forbidden("x".into())).is_empty());
        assert!(!message_for(&Error::Validation("x".into())).is_empty());
    }
}
