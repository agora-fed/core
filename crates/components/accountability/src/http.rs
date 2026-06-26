//! Axum surface for `accountability`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004).
//!
//! **Authorization (SECURITY.md / ADR-0007).** Every *mutating* handler authorizes the
//! authenticated caller through the verified [`dsoc_app::CallerId`] extractor — the acting citizen
//! and org are taken from the request's identity, NEVER from the body. The service re-authorizes
//! against the resource's real org before any write. Reads are public.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::Error;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{Commitment, StatusUpdate};
use crate::queries::Cursor;
use crate::service::{AccountabilityService, DEFAULT_PAGE_LIMIT};

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/accountability/results",
            post(create_result).get(list_results),
        )
        .route("/accountability/results/{id}", get(get_result))
        .route(
            "/accountability/results/{id}/updates",
            post(post_status_update).get(list_status_updates),
        )
        .with_state(state)
}

fn service(state: &AppState) -> AccountabilityService {
    AccountabilityService::from_state(state)
}

// --- DTOs ----------------------------------------------------------------------------

/// Public view of a tracked commitment.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResultDto {
    /// Result id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Short public title of the commitment.
    pub title: String,
    /// The target/outcome being committed to.
    pub target: String,
    /// Lifecycle status (`pending` | `in_progress` | `achieved`).
    pub status: String,
    /// Progress percentage in `0..=100`.
    pub progress: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Commitment> for ResultDto {
    fn from(r: Commitment) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            title: r.title,
            target: r.target,
            status: r.status.as_str().to_owned(),
            progress: r.progress,
            created_at: r.created_at,
        }
    }
}

/// Public view of a status update.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StatusUpdateDto {
    /// Status-update id.
    pub id: Uuid,
    /// The result this update belongs to.
    pub result_id: Uuid,
    /// Human note describing what changed.
    pub note: String,
    /// Progress reported at this point, in `0..=100`.
    pub progress: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<StatusUpdate> for StatusUpdateDto {
    fn from(u: StatusUpdate) -> Self {
        Self {
            id: u.id,
            result_id: u.result_id,
            note: u.note,
            progress: u.progress,
            created_at: u.created_at,
        }
    }
}

/// The response to posting a status update: the persisted update plus the recomputed result.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StatusUpdateResultDto {
    /// The persisted progress update.
    pub update: StatusUpdateDto,
    /// The result after applying the update (its new progress/status).
    pub result: ResultDto,
}

/// Request to create a tracked commitment. The owning org comes from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateResultRequest {
    /// Short public title of the commitment.
    pub title: String,
    /// The target/outcome being committed to (may be open-ended).
    pub target: String,
}

/// Request to post a progress update. The acting citizen comes from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PostStatusUpdateRequest {
    /// Human note describing what changed.
    pub note: String,
    /// Progress reported now, in `0..=100`.
    pub progress: i32,
}

/// Query parameters for keyset-paginated list endpoints (newest-first by `(created_at, id)`).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Organization to scope the result list to (required for `/accountability/results`).
    pub org_id: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
    /// Cursor: the `created_at` of the previous page's last row.
    pub cursor_at: Option<DateTime<Utc>>,
    /// Cursor: the id of the previous page's last row.
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

/// `POST /accountability/results` — create a commitment owned by the caller's org.
///
/// The verified caller (from [`dsoc_app::CallerId`], never the body — ADR-0007) is authorized in the
/// service before any write; the new result is owned by `caller.org`.
async fn create_result(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateResultRequest>,
) -> Response {
    match service(&state)
        .create_result(caller.citizen, caller.org, &req.title, &req.target)
        .await
    {
        Ok(result) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(ResultDto::from(result))),
        )
            .into_response(),
        Err(e) => error_response::<ResultDto>(&e),
    }
}

/// `GET /accountability/results?org_id=&limit=&cursor_at=&cursor_id=` — keyset-paginated list.
async fn list_results(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    let Some(org_id) = params.org_id else {
        return error_response::<Vec<ResultDto>>(&Error::Validation(
            "org_id is required".to_owned(),
        ));
    };
    match service(&state)
        .list_results(OrgId::from_uuid(org_id), params.cursor(), params.limit())
        .await
    {
        Ok(rows) => {
            let dtos: Vec<ResultDto> = rows.into_iter().map(ResultDto::from).collect();
            let meta = page_meta(&dtos, params.limit());
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<ResultDto>>(&e),
    }
}

/// `GET /accountability/results/{id}` — fetch one commitment (public read).
async fn get_result(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match service(&state).get_result(id).await {
        Ok(result) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ResultDto::from(result))),
        )
            .into_response(),
        Err(e) => error_response::<ResultDto>(&e),
    }
}

/// `POST /accountability/results/{id}/updates` — post a progress update (authorized).
///
/// The verified caller (from [`dsoc_app::CallerId`], never the body — ADR-0007) is authorized
/// against the result's real org in the service before any write.
async fn post_status_update(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<PostStatusUpdateRequest>,
) -> Response {
    match service(&state)
        .post_status_update(caller.citizen, id, &req.note, req.progress)
        .await
    {
        Ok((result, update)) => {
            let body = StatusUpdateResultDto {
                update: StatusUpdateDto::from(update),
                result: ResultDto::from(result),
            };
            (StatusCode::CREATED, Json(ApiResponse::ok(body))).into_response()
        }
        Err(e) => error_response::<StatusUpdateResultDto>(&e),
    }
}

/// `GET /accountability/results/{id}/updates?limit=&cursor_at=&cursor_id=` — keyset-paginated
/// updates (public read).
async fn list_status_updates(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ListParams>,
) -> Response {
    match service(&state)
        .list_status_updates(id, params.cursor(), params.limit())
        .await
    {
        Ok(rows) => {
            let dtos: Vec<StatusUpdateDto> = rows.into_iter().map(StatusUpdateDto::from).collect();
            let meta = page_meta(&dtos, params.limit());
            (StatusCode::OK, Json(ApiResponse::page(dtos, meta))).into_response()
        }
        Err(e) => error_response::<Vec<StatusUpdateDto>>(&e),
    }
}

// --- helpers -------------------------------------------------------------------------

/// Keyset pagination has no cheap total; report the page length as a lower bound.
fn page_meta<T>(items: &[T], limit: i64) -> PageMeta {
    PageMeta {
        total: items.len() as u64,
        limit: u32::try_from(limit.clamp(1, 200)).unwrap_or(DEFAULT_PAGE_LIMIT as u32),
        offset: 0,
    }
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no internals.
fn error_response<T: Serialize>(err: &Error) -> Response {
    let status = status_for(err);
    let body: ApiResponse<T> = ApiResponse::fail(err.code(), message_pt(err));
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

/// Map a canonical error to a Portuguese, public-safe end-user message.
fn message_pt(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Registro de accountability não encontrado.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Operação em conflito com o estado atual do registro.",
        _ => "Erro interno ao processar a requisição.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Status;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn result_dto_maps_fields_and_status_token() {
        let r = Commitment {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "Construir a creche".to_owned(),
            target: "Entrega em 2026".to_owned(),
            status: Status::InProgress,
            progress: 40,
            created_at: at(),
        };
        let dto = ResultDto::from(r.clone());
        assert_eq!(dto.id, r.id);
        assert_eq!(dto.status, "in_progress");
        assert_eq!(dto.progress, 40);
        assert_eq!(dto.title, "Construir a creche");
    }

    #[test]
    fn status_update_dto_maps_fields() {
        let u = StatusUpdate {
            id: Uuid::now_v7(),
            result_id: Uuid::now_v7(),
            note: "fundação concluída".to_owned(),
            progress: 25,
            created_at: at(),
        };
        let dto = StatusUpdateDto::from(u.clone());
        assert_eq!(dto.id, u.id);
        assert_eq!(dto.result_id, u.result_id);
        assert_eq!(dto.progress, 25);
        assert_eq!(dto.note, "fundação concluída");
    }

    #[test]
    fn list_params_builds_cursor_only_when_both_present() {
        let none = ListParams {
            org_id: Some(Uuid::now_v7()),
            limit: None,
            cursor_at: Some(at()),
            cursor_id: None,
        };
        assert!(none.cursor().is_none());
        assert_eq!(none.limit(), DEFAULT_PAGE_LIMIT);

        let both = ListParams {
            org_id: None,
            limit: Some(10),
            cursor_at: Some(at()),
            cursor_id: Some(Uuid::now_v7()),
        };
        assert!(both.cursor().is_some());
        assert_eq!(both.limit(), 10);
    }

    #[test]
    fn page_meta_reports_length_as_lower_bound() {
        let meta = page_meta(&[1u32, 2, 3], 50);
        assert_eq!(meta.total, 3);
        assert_eq!(meta.limit, 50);
        assert_eq!(meta.offset, 0);
        // A degenerate limit clamps up to one.
        let degenerate = page_meta::<u32>(&[], 0);
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
            status_for(&Error::Storage(Box::new(std::io::Error::other("secret")))),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn messages_are_public_safe_portuguese() {
        let storage = Error::Storage(Box::new(std::io::Error::other("connection string")));
        assert_eq!(
            message_pt(&storage),
            "Erro interno ao processar a requisição."
        );
        assert_eq!(message_pt(&Error::Unauthorized), "Autenticação necessária.");
        assert_eq!(
            message_pt(&Error::Conflict("x".into())),
            "Operação em conflito com o estado atual do registro."
        );
    }
}
