//! Axum surface for `budgets`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004).
//!
//! **Authorization (SECURITY.md / ADR-0007).** Every *mutating* handler authorizes the
//! authenticated caller through the verified [`dsoc_app::CallerId`] extractor — the acting
//! citizen and org are taken from the request's identity, NEVER from the body — before any
//! write. Setting up a budget round (creating a budget or a project) is an organizer action
//! and requires directory-level verification; placing an order is a citizen action and
//! requires an email-confirmed citizen. Reads are public.

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

use crate::domain::{NewBudget, NewProject};
use crate::queries::{BudgetRow, OrderRow, ProjectRow};
use crate::service::{BudgetService, PlacedOrder, DEFAULT_PAGE_LIMIT};

/// Assurance required to set up a budget round (create a budget or add a project): an
/// organizer verified against an official directory.
const ORGANIZER_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Assurance required to place an order: an email-confirmed citizen.
const PARTICIPANT_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/budgets", post(create_budget).get(list_budgets))
        .route("/budgets/{id}", get(get_budget))
        .route(
            "/budgets/{id}/projects",
            post(add_project).get(list_projects),
        )
        .route("/budgets/{id}/orders", post(place_order).get(list_orders))
        .with_state(state)
}

// --- DTOs ----------------------------------------------------------------------------

/// Public view of a budget.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BudgetDto {
    /// Budget id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title of the budgeting round.
    pub title: String,
    /// Spending ceiling, in cents.
    pub ceiling_cents: i64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<BudgetRow> for BudgetDto {
    fn from(row: BudgetRow) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            ceiling_cents: row.ceiling_cents,
            created_at: row.created_at,
        }
    }
}

/// Public view of a project.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectDto {
    /// Project id.
    pub id: Uuid,
    /// The budget this project belongs to.
    pub budget_id: Uuid,
    /// Title of the project.
    pub title: String,
    /// Cost, in cents.
    pub cost_cents: i64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<ProjectRow> for ProjectDto {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: row.id,
            budget_id: row.budget_id,
            title: row.title,
            cost_cents: row.cost_cents,
            created_at: row.created_at,
        }
    }
}

/// Public view of an order (header only).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrderDto {
    /// Order id.
    pub id: Uuid,
    /// The budget this order allocates within.
    pub budget_id: Uuid,
    /// The citizen who placed the order.
    pub citizen_id: Uuid,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<OrderRow> for OrderDto {
    fn from(row: OrderRow) -> Self {
        Self {
            id: row.id,
            budget_id: row.budget_id,
            citizen_id: row.citizen_id,
            created_at: row.created_at,
        }
    }
}

/// Public view of a placed order: the header plus the total allocated and the selected
/// projects (the allocation total is computed server-side, never trusted from the client).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlacedOrderDto {
    /// The stored order header.
    pub order: OrderDto,
    /// Total allocated, in cents (`<= budget ceiling`).
    pub allocated_cents: i64,
    /// The project ids this order allocated to, ascending.
    pub project_ids: Vec<Uuid>,
}

impl From<PlacedOrder> for PlacedOrderDto {
    fn from(placed: PlacedOrder) -> Self {
        Self {
            order: OrderDto::from(placed.order),
            allocated_cents: placed.allocated_cents,
            project_ids: placed.project_ids,
        }
    }
}

/// Request to create a budget. The owning org comes from the verified [`dsoc_app::CallerId`]
/// (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateBudgetRequest {
    /// Title of the budgeting round (Portuguese civic content).
    pub title: String,
    /// Spending ceiling, in cents (must be positive).
    pub ceiling_cents: i64,
}

/// Request to add a project to a budget.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddProjectRequest {
    /// Title of the project.
    pub title: String,
    /// Cost, in cents (must be positive).
    pub cost_cents: i64,
}

/// Request to place an order against a budget. The acting citizen comes from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PlaceOrderRequest {
    /// The projects this order allocates to (their costs must sum within the ceiling).
    pub project_ids: Vec<Uuid>,
}

/// Query parameters for listing budgets (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct BudgetListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return budgets with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

/// Query parameters for listing a budget's projects or orders.
#[derive(Debug, Clone, Deserialize)]
pub struct ChildListParams {
    /// Keyset cursor over ascending id. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

// --- handlers ------------------------------------------------------------------------

/// `POST /budgets` — open a budgeting round owned by the caller's org (organizer action).
async fn create_budget(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateBudgetRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, ORGANIZER_LEVEL)
        .await
    {
        return error_response::<BudgetDto>(&e);
    }
    let new = match NewBudget::validate(&req.title, req.ceiling_cents) {
        Ok(n) => n,
        Err(e) => return error_response::<BudgetDto>(&e),
    };
    let svc = BudgetService::from_state(&state);
    match svc.create_budget(caller.org, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(BudgetDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<BudgetDto>(&e),
    }
}

/// `GET /budgets/{id}` — fetch one budget (public read).
async fn get_budget(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = BudgetService::from_state(&state);
    match svc.get_budget(id).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(BudgetDto::from(row)))).into_response(),
        Err(e) => error_response::<BudgetDto>(&e),
    }
}

/// `GET /budgets?org_id=&after=&limit=` — keyset-paginated budget list (public read).
async fn list_budgets(
    State(state): State<AppState>,
    Query(params): Query<BudgetListParams>,
) -> Response {
    let svc = BudgetService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_budgets(org, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<BudgetDto> = rows.into_iter().map(BudgetDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<BudgetDto>>(&e),
    }
}

/// `POST /budgets/{id}/projects` — add a fundable project to a budget (organizer action).
async fn add_project(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<AddProjectRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, ORGANIZER_LEVEL)
        .await
    {
        return error_response::<ProjectDto>(&e);
    }
    let new = match NewProject::validate(&req.title, req.cost_cents) {
        Ok(n) => n,
        Err(e) => return error_response::<ProjectDto>(&e),
    };
    let svc = BudgetService::from_state(&state);
    match svc.add_project(id, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(ProjectDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<ProjectDto>(&e),
    }
}

/// `GET /budgets/{id}/projects?after=&limit=` — keyset-paginated project list (public read).
async fn list_projects(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ChildListParams>,
) -> Response {
    let svc = BudgetService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_projects(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<ProjectDto> = rows.into_iter().map(ProjectDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<ProjectDto>>(&e),
    }
}

/// `POST /budgets/{id}/orders` — place the caller's allocation order under the ceiling.
///
/// Placing an order is a citizen action; the verified caller (from [`dsoc_app::CallerId`],
/// never the body — ADR-0007) is authorized (email-verified) before any write and recorded as
/// the order's citizen. The allocation rule is enforced server-side by [`BudgetService`].
async fn place_order(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<PlaceOrderRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, PARTICIPANT_LEVEL)
        .await
    {
        return error_response::<PlacedOrderDto>(&e);
    }
    let svc = BudgetService::from_state(&state);
    match svc.place_order(id, caller.citizen, &req.project_ids).await {
        Ok(placed) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(PlacedOrderDto::from(placed))),
        )
            .into_response(),
        Err(e) => error_response::<PlacedOrderDto>(&e),
    }
}

/// `GET /budgets/{id}/orders?after=&limit=` — keyset-paginated order list (public read).
async fn list_orders(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ChildListParams>,
) -> Response {
    let svc = BudgetService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_orders(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<OrderDto> = rows.into_iter().map(OrderDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<OrderDto>>(&e),
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

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no
/// internals.
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
    fn budget_dto_maps_fields() {
        let row = BudgetRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "Orçamento 2026".to_string(),
            ceiling_cents: 1_000_000,
            created_at: at(),
        };
        let dto = BudgetDto::from(row.clone());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.title, "Orçamento 2026");
        assert_eq!(dto.ceiling_cents, 1_000_000);
    }

    #[test]
    fn project_dto_maps_fields() {
        let row = ProjectRow {
            id: Uuid::now_v7(),
            budget_id: Uuid::now_v7(),
            title: "Praça".to_string(),
            cost_cents: 50_000,
            created_at: at(),
        };
        let dto = ProjectDto::from(row.clone());
        assert_eq!(dto.budget_id, row.budget_id);
        assert_eq!(dto.cost_cents, 50_000);
    }

    #[test]
    fn placed_order_dto_carries_total_and_projects() {
        let pid = Uuid::now_v7();
        let placed = PlacedOrder {
            order: OrderRow {
                id: Uuid::now_v7(),
                budget_id: Uuid::now_v7(),
                citizen_id: Uuid::now_v7(),
                created_at: at(),
            },
            allocated_cents: 700,
            project_ids: vec![pid],
        };
        let dto = PlacedOrderDto::from(placed.clone());
        assert_eq!(dto.allocated_cents, 700);
        assert_eq!(dto.project_ids, vec![pid]);
        assert_eq!(dto.order.id, placed.order.id);
    }

    #[test]
    fn page_meta_clamps_and_counts() {
        let meta = page_meta(7, 20);
        assert_eq!(meta.total, 7);
        assert_eq!(meta.limit, 20);
        assert_eq!(meta.offset, 0);
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

    #[test]
    fn required_levels_are_ordered_organizer_above_participant() {
        assert!(ORGANIZER_LEVEL > PARTICIPANT_LEVEL);
    }
}
