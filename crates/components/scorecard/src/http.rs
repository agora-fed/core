//! Axum HTTP surface for the scorecard. Reads are public (the scorecard IS the public accountability
//! artifact); the two promise mutations require an authenticated, directory-verified official.
//!
//! The crate exposes [`routes`] for the gateway to mount; it never binds a socket (ADR-0004 — the
//! gateway owns the IPv6 bind). Handlers map the domain onto the shared
//! [`dsoc_api_contract::ScorecardDto`] (and an owned promise DTO) wrapped in the [`ApiResponse`]
//! envelope, and translate [`dsoc_core::Error`] into status codes without leaking internals.
//!
//! The acting citizen id is taken ONLY from the gateway-injected `x-citizen-id` header (the
//! authenticated caller) — never from the request body (UNIVERSAL HARD RULES / SECURITY.md).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta, ScorecardDto};
use dsoc_app::AppState;
use dsoc_core::error::Error;
use dsoc_core::ids::{CitizenId, MandateId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{Promise, ScorecardView};
use crate::queries::Cursor;
use crate::service::{ScorecardService, DEFAULT_PAGE_LIMIT};

/// HTTP header carrying the acting citizen's id (injected by the gateway's auth layer).
const ACTOR_HEADER: &str = "x-citizen-id";

/// Build the scorecard router for the gateway to mount.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/scorecards", get(list_scorecards))
        .route("/scorecards/{mandate_id}", get(get_scorecard))
        .route(
            "/scorecards/{mandate_id}/promises",
            get(list_promises).post(record_promise),
        )
        .route(
            "/scorecards/promises/{promise_id}/deliver",
            post(deliver_promise),
        )
        .with_state(state)
}

fn service(state: &AppState) -> ScorecardService {
    ScorecardService::from_state(state)
}

// --- DTOs ---------------------------------------------------------------------------

/// Map the projection view onto the frozen public [`ScorecardDto`]. The i64 counters are
/// non-negative by construction (CHECK constraints + ledger recount), so the lossless
/// `u64` conversion below never clamps in practice.
fn scorecard_dto(view: &ScorecardView) -> ScorecardDto {
    ScorecardDto {
        mandate_id: view.mandate_id,
        answered: u64::try_from(view.answered.max(0)).unwrap_or(0),
        ignored: u64::try_from(view.ignored.max(0)).unwrap_or(0),
        median_response_hours: view.median_response_hours,
    }
}

/// Public view of a promise ("promises vs delivery").
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromiseDto {
    /// Promise id.
    pub id: Uuid,
    /// The scorecard this promise belongs to.
    pub scorecard_id: Uuid,
    /// The public promise text (Portuguese civic content).
    pub text: String,
    /// When the promise was made.
    pub made_at: DateTime<Utc>,
    /// Whether the politician delivered on it.
    pub delivered: bool,
    /// When it was marked delivered (absent until delivered).
    pub delivered_at: Option<DateTime<Utc>>,
}

impl From<Promise> for PromiseDto {
    fn from(p: Promise) -> Self {
        Self {
            id: p.id,
            scorecard_id: p.scorecard_id,
            text: p.text,
            made_at: p.made_at,
            delivered: p.delivered,
            delivered_at: p.delivered_at,
        }
    }
}

/// Request body to record a public promise. The mandate comes from the path and the acting official
/// from the authenticated header — never from this body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RecordPromiseRequest {
    /// The public promise text.
    pub text: String,
}

/// Query parameters for keyset-paginated list endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Organization to scope the scorecard list to (required for `/scorecards`).
    pub org_id: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
    /// Cursor: the ordering timestamp of the previous page's last row.
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

// --- handlers -----------------------------------------------------------------------

/// `GET /scorecards?org_id=&limit=&cursor_at=&cursor_id=` — keyset-paginated public scorecards.
async fn list_scorecards(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<ScorecardDto>>>, ApiFailure> {
    let org_id = params
        .org_id
        .ok_or_else(|| ApiFailure(Error::Validation("org_id is required".to_owned())))?;
    let views = service(&state)
        .list(
            dsoc_core::ids::OrgId::from_uuid(org_id),
            params.cursor(),
            params.limit(),
        )
        .await?;
    let dtos: Vec<ScorecardDto> = views.iter().map(scorecard_dto).collect();
    Ok(Json(paged(dtos, params.limit())))
}

/// `GET /scorecards/{mandate_id}` — one politician's public scorecard.
async fn get_scorecard(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
) -> Result<Json<ApiResponse<ScorecardDto>>, ApiFailure> {
    let view = service(&state)
        .get_by_mandate(MandateId::from_uuid(mandate_id))
        .await?;
    Ok(Json(ApiResponse::ok(scorecard_dto(&view))))
}

/// `GET /scorecards/{mandate_id}/promises` — keyset-paginated public promises.
async fn list_promises(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<PromiseDto>>>, ApiFailure> {
    let promises = service(&state)
        .list_promises(
            MandateId::from_uuid(mandate_id),
            params.cursor(),
            params.limit(),
        )
        .await?;
    let dtos: Vec<PromiseDto> = promises.into_iter().map(PromiseDto::from).collect();
    Ok(Json(paged(dtos, params.limit())))
}

/// `POST /scorecards/{mandate_id}/promises` — record a public promise (directory-verified official).
async fn record_promise(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<RecordPromiseRequest>,
) -> Result<(StatusCode, Json<ApiResponse<PromiseDto>>), ApiFailure> {
    let actor = actor_from_headers(&headers)?;
    let promise = service(&state)
        .record_promise(actor, MandateId::from_uuid(mandate_id), &req.text)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(PromiseDto::from(promise))),
    ))
}

/// `POST /scorecards/promises/{promise_id}/deliver` — mark a promise delivered (official).
async fn deliver_promise(
    State(state): State<AppState>,
    Path(promise_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<PromiseDto>>, ApiFailure> {
    let actor = actor_from_headers(&headers)?;
    let promise = service(&state)
        .mark_promise_delivered(actor, promise_id)
        .await?;
    Ok(Json(ApiResponse::ok(PromiseDto::from(promise))))
}

// --- helpers ------------------------------------------------------------------------

fn paged<T>(items: Vec<T>, limit: i64) -> ApiResponse<Vec<T>> {
    // Keyset pagination has no cheap total; report the page length as a lower bound.
    let meta = PageMeta {
        total: items.len() as u64,
        limit: u32::try_from(limit.max(0)).unwrap_or(u32::MAX),
        offset: 0,
    };
    ApiResponse::page(items, meta)
}

/// Extract the acting citizen id from the gateway-injected header. Absent/malformed -> 401.
fn actor_from_headers(headers: &HeaderMap) -> Result<CitizenId, Error> {
    let raw = headers
        .get(ACTOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Unauthorized)?;
    let id = Uuid::parse_str(raw).map_err(|_| Error::Unauthorized)?;
    Ok(CitizenId::from_uuid(id))
}

/// Wraps a [`dsoc_core::Error`] so it renders as a uniform [`ApiResponse`] failure with the right
/// HTTP status. Never leaks internal detail (storage errors render generically).
#[derive(Debug)]
pub struct ApiFailure(Error);

impl From<Error> for ApiFailure {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let status = status_for(&self.0);
        let body = ApiResponse::<()>::fail(self.0.code(), message_pt(&self.0));
        (status, Json(body)).into_response()
    }
}

fn status_for(err: &Error) -> StatusCode {
    match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Conflict(_) => StatusCode::CONFLICT,
        // Storage, Dependency, and any future non-exhaustive variant are internal concerns.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Map a canonical error to a Portuguese, public-safe end-user message.
fn message_pt(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Placar não encontrado.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Registro já existe.",
        _ => "Erro interno ao processar o placar.",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn error_status_mapping_is_stable() {
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
            status_for(&Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        let storage = Error::Storage(Box::new(std::io::Error::other("secret")));
        assert_eq!(status_for(&storage), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn messages_are_public_safe_portuguese() {
        let storage = Error::Storage(Box::new(std::io::Error::other("connection string")));
        assert_eq!(message_pt(&storage), "Erro interno ao processar o placar.");
        assert_eq!(message_pt(&Error::Unauthorized), "Autenticação necessária.");
    }

    #[test]
    fn scorecard_dto_maps_view_and_clamps_non_negative() {
        let view = ScorecardView {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            answered: 3,
            ignored: 5,
            median_response_hours: Some(6.0),
            created_at: at(),
            updated_at: at(),
        };
        let dto = scorecard_dto(&view);
        assert_eq!(dto.mandate_id, view.mandate_id);
        assert_eq!(dto.answered, 3);
        assert_eq!(dto.ignored, 5);
        assert_eq!(dto.median_response_hours, Some(6.0));
    }

    #[test]
    fn promise_dto_maps_fields() {
        let p = Promise {
            id: Uuid::now_v7(),
            scorecard_id: Uuid::now_v7(),
            text: "construir a creche".to_owned(),
            made_at: at(),
            delivered: true,
            delivered_at: Some(at()),
            created_at: at(),
        };
        let dto = PromiseDto::from(p.clone());
        assert_eq!(dto.id, p.id);
        assert!(dto.delivered);
        assert_eq!(dto.delivered_at, Some(at()));
    }

    #[test]
    fn actor_from_headers_requires_valid_uuid() {
        assert!(actor_from_headers(&HeaderMap::new()).is_err());
        let mut bad = HeaderMap::new();
        bad.insert(ACTOR_HEADER, HeaderValue::from_static("not-a-uuid"));
        assert!(actor_from_headers(&bad).is_err());
        let id = CitizenId::new();
        let mut good = HeaderMap::new();
        good.insert(
            ACTOR_HEADER,
            HeaderValue::from_str(&id.as_uuid().to_string()).unwrap(),
        );
        assert_eq!(actor_from_headers(&good).unwrap(), id);
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
    fn paged_reports_length_as_lower_bound() {
        let resp = paged(vec![1u32, 2, 3], 50);
        let meta = resp.meta.unwrap();
        assert_eq!(meta.total, 3);
        assert_eq!(meta.limit, 50);
        assert_eq!(meta.offset, 0);
    }
}
