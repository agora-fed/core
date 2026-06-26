//! Axum HTTP surface for `dsoc-processes`. Exposes `pub fn routes(state: AppState) -> Router<()>`
//! (ADR-0004 wiring); it never binds a socket — the gateway owns the IPv6 bind. Handlers map the
//! domain onto JSON DTOs wrapped in the shared [`ApiResponse`] envelope, and map `dsoc_core::Error`
//! to an HTTP status without leaking internal detail (SECURITY.md).
//!
//! AUTHORIZATION (ADR-0007): every mutating handler (create, update, delete, add phase, advance
//! phase) resolves the AUTHENTICATED caller from the [`dsoc_app::CallerId`] extractor — the verified
//! identity the gateway sets via the trusted `x-dsoc-citizen-id` / `x-dsoc-org-id` headers (which
//! the public ingress strips). It authorizes that caller via the injected `Arc<dyn Authorization>`
//! inside the service and NEVER trusts an org/citizen id taken from the request body.

use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use dsoc_api_contract::envelope::ApiResponse;
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::Error;

use crate::domain::ProcessStatus;
use crate::service::{Phase, Process, ProcessDraft, ProcessService, ProcessUpdate};

// ---------------------------------------------------------------------------
// Response DTOs (this crate owns its shapes; the gateway composes /openapi.json).
// ---------------------------------------------------------------------------

/// Public view of a participatory process.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProcessDto {
    /// Process id.
    pub id: Uuid,
    /// Organization id.
    pub org_id: Uuid,
    /// Stable URL-safe handle.
    pub slug: String,
    /// Human title.
    pub title: String,
    /// Lifecycle state (`draft` | `active` | `closed`).
    pub status: String,
    /// Participation window start, if scheduled.
    pub starts_at: Option<DateTime<Utc>>,
    /// Participation window end, if scheduled.
    pub ends_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Process> for ProcessDto {
    fn from(value: Process) -> Self {
        Self {
            id: value.id,
            org_id: value.org.as_uuid(),
            slug: value.slug,
            title: value.title,
            status: value.status.as_str().to_owned(),
            starts_at: value.starts_at,
            ends_at: value.ends_at,
            created_at: value.created_at,
        }
    }
}

/// Public view of a process phase.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PhaseDto {
    /// Phase id.
    pub id: Uuid,
    /// The process this phase belongs to.
    pub process_id: Uuid,
    /// Human name.
    pub name: String,
    /// Zero-based ordinal within the process.
    pub position: i32,
    /// Whether this phase is the currently open one.
    pub active: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Phase> for PhaseDto {
    fn from(value: Phase) -> Self {
        Self {
            id: value.id,
            process_id: value.process_id,
            name: value.name,
            position: value.position,
            active: value.active,
            created_at: value.created_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Request DTOs. The acting citizen/org is NEVER read from the body (ADR-0007).
// ---------------------------------------------------------------------------

/// `POST /processes` body: create a participatory process.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateProcessRequest {
    /// Stable URL-safe handle (`[a-z0-9._-]`), unique within the org.
    pub slug: String,
    /// Human title.
    pub title: String,
    /// Initial lifecycle state (`draft` | `active` | `closed`); defaults to `draft`.
    pub status: Option<String>,
    /// Participation window start, if known.
    pub starts_at: Option<DateTime<Utc>>,
    /// Participation window end, if known.
    pub ends_at: Option<DateTime<Utc>>,
}

/// `PUT /processes/{id}` body: partial update.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateProcessRequest {
    /// New title, if changing.
    pub title: Option<String>,
    /// New lifecycle state (`draft` | `active` | `closed`), if changing.
    pub status: Option<String>,
}

/// `POST /processes/{id}/phases` body: add an ordered phase.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddPhaseRequest {
    /// Human name of the phase.
    pub name: String,
    /// Zero-based ordinal within the process (unique).
    pub position: i32,
}

/// Query parameters carrying the organization/tenant for a read.
#[derive(Debug, Clone, Deserialize)]
pub struct OrgQuery {
    /// The organization the process belongs to.
    pub org_id: Uuid,
}

/// Keyset pagination query for the process list (with the org).
#[derive(Debug, Clone, Deserialize)]
pub struct ListProcessQuery {
    /// The organization the processes belong to.
    pub org_id: Uuid,
    /// Return processes strictly after this id (keyset cursor).
    pub after: Option<Uuid>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

/// Keyset pagination query for a process's phase list.
#[derive(Debug, Clone, Deserialize)]
pub struct ListPhaseQuery {
    /// Return phases strictly after this position (keyset cursor).
    pub after: Option<i32>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the participatory-process routes onto a router carrying the shared [`AppState`].
///
/// Reads are open (org scoped by query); mutations require a verified caller (`CallerId` +
/// authorization inside [`ProcessService`]).
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/processes", post(create_process).get(list_processes))
        .route(
            "/processes/{process_id}",
            get(get_process).put(update_process).delete(delete_process),
        )
        .route(
            "/processes/{process_id}/phases",
            post(add_phase).get(list_phases),
        )
        .route(
            "/processes/{process_id}/phases/advance",
            post(advance_phase),
        )
        .with_state(state)
}

async fn create_process(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<CreateProcessRequest>,
) -> Result<(StatusCode, Json<ApiResponse<ProcessDto>>), ApiErr> {
    let status = body
        .status
        .as_deref()
        .map(ProcessStatus::parse)
        .transpose()?;
    let svc = ProcessService::from_state(&state);
    let created = svc
        .create_process(
            caller.org,
            caller.citizen,
            ProcessDraft {
                slug: body.slug,
                title: body.title,
                status,
                starts_at: body.starts_at,
                ends_at: body.ends_at,
            },
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(ProcessDto::from(created))),
    ))
}

async fn get_process(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> Result<Json<ApiResponse<ProcessDto>>, ApiErr> {
    let svc = ProcessService::from_state(&state);
    let process = svc
        .get_process(OrgId::from_uuid(query.org_id), process_id)
        .await?;
    Ok(Json(ApiResponse::ok(ProcessDto::from(process))))
}

async fn list_processes(
    State(state): State<AppState>,
    Query(query): Query<ListProcessQuery>,
) -> Result<Json<ApiResponse<Vec<ProcessDto>>>, ApiErr> {
    let svc = ProcessService::from_state(&state);
    let processes = svc
        .list_processes(OrgId::from_uuid(query.org_id), query.after, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        processes.into_iter().map(ProcessDto::from).collect(),
    )))
}

async fn update_process(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    caller: CallerId,
    Json(body): Json<UpdateProcessRequest>,
) -> Result<Json<ApiResponse<ProcessDto>>, ApiErr> {
    let status = body
        .status
        .as_deref()
        .map(ProcessStatus::parse)
        .transpose()?;
    let svc = ProcessService::from_state(&state);
    let updated = svc
        .update_process(
            caller.org,
            caller.citizen,
            process_id,
            ProcessUpdate {
                title: body.title,
                status,
            },
        )
        .await?;
    Ok(Json(ApiResponse::ok(ProcessDto::from(updated))))
}

async fn delete_process(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    caller: CallerId,
) -> Result<StatusCode, ApiErr> {
    let svc = ProcessService::from_state(&state);
    svc.delete_process(caller.org, caller.citizen, process_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_phase(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    caller: CallerId,
    Json(body): Json<AddPhaseRequest>,
) -> Result<(StatusCode, Json<ApiResponse<PhaseDto>>), ApiErr> {
    let svc = ProcessService::from_state(&state);
    let phase = svc
        .add_phase(
            caller.org,
            caller.citizen,
            process_id,
            &body.name,
            body.position,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(PhaseDto::from(phase))),
    ))
}

async fn list_phases(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    Query(query): Query<ListPhaseQuery>,
) -> Result<Json<ApiResponse<Vec<PhaseDto>>>, ApiErr> {
    let svc = ProcessService::from_state(&state);
    let phases = svc
        .list_phases(process_id, query.after, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        phases.into_iter().map(PhaseDto::from).collect(),
    )))
}

async fn advance_phase(
    State(state): State<AppState>,
    Path(process_id): Path<Uuid>,
    caller: CallerId,
) -> Result<Json<ApiResponse<PhaseDto>>, ApiErr> {
    let svc = ProcessService::from_state(&state);
    let phase = svc
        .advance_phase(caller.org, caller.citizen, process_id)
        .await?;
    Ok(Json(ApiResponse::ok(PhaseDto::from(phase))))
}

/// Newtype adapting [`dsoc_core::Error`] into an HTTP response with the right status code and a
/// public-safe, Portuguese end-user message (never leaks internals; coding-style / security).
struct ApiErr(Error);

impl From<Error> for ApiErr {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        if matches!(self.0, Error::Storage(_) | Error::Dependency { .. }) {
            tracing::error!(code = self.0.code(), detail = %self.0, "processes request failed");
        }
        let body = ApiResponse::<()>::fail(self.0.code(), message_pt(&self.0));
        (status_for(&self.0), Json(body)).into_response()
    }
}

/// Map a canonical error to its HTTP status (stable, public-safe).
fn status_for(error: &Error) -> StatusCode {
    match error {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Conflict(_) => StatusCode::CONFLICT,
        // Storage, Dependency, and any future non-exhaustive variant are internal.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Map a canonical error to a Portuguese, public-safe end-user message.
fn message_pt(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Processo ou fase não encontrado.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Conflito de estado do processo.",
        // Storage, Dependency, and any future non-exhaustive variant: generic internal message.
        _ => "Erro interno ao processar o processo participativo.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn process_dto_maps_from_domain() {
        let org = OrgId::new();
        let id = Uuid::now_v7();
        let dto = ProcessDto::from(Process {
            id,
            org,
            slug: "orcamento-2026".to_owned(),
            title: "Orçamento 2026".to_owned(),
            status: ProcessStatus::Active,
            starts_at: Some(ts()),
            ends_at: None,
            created_at: ts(),
        });
        assert_eq!(dto.id, id);
        assert_eq!(dto.org_id, org.as_uuid());
        assert_eq!(dto.status, "active");
        assert_eq!(dto.slug, "orcamento-2026");
    }

    #[test]
    fn phase_dto_maps_from_domain() {
        let dto = PhaseDto::from(Phase {
            id: Uuid::now_v7(),
            process_id: Uuid::now_v7(),
            name: "Votação".to_owned(),
            position: 2,
            active: true,
            created_at: ts(),
        });
        assert_eq!(dto.name, "Votação");
        assert_eq!(dto.position, 2);
        assert!(dto.active);
    }

    #[test]
    fn error_status_codes_are_mapped() {
        let cases = [
            (Error::NotFound("x".to_owned()), StatusCode::NOT_FOUND),
            (Error::Forbidden("x".to_owned()), StatusCode::FORBIDDEN),
            (Error::Unauthorized, StatusCode::UNAUTHORIZED),
            (
                Error::Validation("x".to_owned()),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (Error::Conflict("x".to_owned()), StatusCode::CONFLICT),
            (
                Error::Storage(Box::new(std::io::Error::other("boom"))),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(status_for(&err), expected);
        }
    }

    #[test]
    fn messages_are_public_safe_and_present() {
        for err in [
            Error::NotFound("x".to_owned()),
            Error::Forbidden("x".to_owned()),
            Error::Unauthorized,
            Error::Validation("x".to_owned()),
            Error::Conflict("x".to_owned()),
            Error::Storage(Box::new(std::io::Error::other("secret detail"))),
        ] {
            let msg = message_pt(&err);
            assert!(!msg.is_empty());
            assert!(!msg.contains("secret detail"), "must not leak internals");
        }
    }
}
