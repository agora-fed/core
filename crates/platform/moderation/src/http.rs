//! Axum HTTP surface for moderation. Handlers map the domain onto owned DTOs wrapped in
//! the shared [`ApiResponse`] envelope, and translate [`dsoc_core::Error`] into status
//! codes. The crate exposes [`routes`] for the gateway to mount; it never binds a socket
//! (ADR-0004 — the gateway owns the IPv6 bind).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::AppState;
use dsoc_core::error::Error;
use dsoc_core::ids::OrgId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{Appeal, AppealStatus, Decision, Rule, RuleAction, RuleKind};
use crate::queries::Cursor;
use crate::service::{ModerationService, DEFAULT_PAGE_LIMIT};

/// Build the moderation router for the gateway to mount.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/moderation/rules", post(create_rule).get(list_rules))
        .route("/moderation/decisions", get(list_decisions))
        .route("/moderation/decisions/{id}", get(get_decision))
        .route("/moderation/appeals", post(file_appeal))
        .route("/moderation/appeals/{id}/resolve", post(resolve_appeal))
        .with_state(state)
}

fn service(state: &AppState) -> ModerationService {
    ModerationService::new(state.db.clone(), state.clock.clone(), state.bus.clone())
}

// --- DTOs ---------------------------------------------------------------------------

/// Public view of a moderation rule.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RuleDto {
    /// Rule id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Matcher kind (`keyword` | `caps_ratio`).
    pub kind: String,
    /// Matcher parameter.
    pub pattern: String,
    /// Prescribed action (`flag` | `reject`).
    pub action: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Rule> for RuleDto {
    fn from(r: Rule) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            kind: r.kind.as_str().to_owned(),
            pattern: r.pattern,
            action: r.action.as_str().to_owned(),
            created_at: r.created_at,
        }
    }
}

/// Public view of a moderation decision (audit record).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DecisionDto {
    /// Decision id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Target kind (`proposal` | `comment`).
    pub target_kind: String,
    /// Target id.
    pub target_id: Uuid,
    /// The rule that fired (absent when cleared).
    pub rule_id: Option<Uuid>,
    /// Outcome (`flagged` | `cleared`).
    pub outcome: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<Decision> for DecisionDto {
    fn from(d: Decision) -> Self {
        Self {
            id: d.id,
            org_id: d.org_id,
            target_kind: d.target_kind.as_str().to_owned(),
            target_id: d.target_id,
            rule_id: d.rule_id,
            outcome: d.outcome.as_str().to_owned(),
            created_at: d.created_at,
        }
    }
}

/// Public view of an appeal.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AppealDto {
    /// Appeal id.
    pub id: Uuid,
    /// The challenged decision.
    pub decision_id: Uuid,
    /// Stated reason.
    pub reason: String,
    /// Status (`open` | `granted` | `denied`).
    pub status: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last transition time.
    pub updated_at: DateTime<Utc>,
}

impl From<Appeal> for AppealDto {
    fn from(a: Appeal) -> Self {
        Self {
            id: a.id,
            decision_id: a.decision_id,
            reason: a.reason,
            status: a.status.as_str().to_owned(),
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

/// Request body to create a rule.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateRuleRequest {
    /// Owning organization.
    pub org_id: Uuid,
    /// Matcher kind (`keyword` | `caps_ratio`).
    pub kind: String,
    /// Matcher parameter.
    pub pattern: String,
    /// Prescribed action (`flag` | `reject`).
    pub action: String,
}

/// Request body to file an appeal.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct FileAppealRequest {
    /// The decision being challenged.
    pub decision_id: Uuid,
    /// The citizen's stated reason.
    pub reason: String,
}

/// Request body to resolve an appeal.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ResolveAppealRequest {
    /// Target status (`granted` | `denied`).
    pub status: String,
}

/// Query parameters for keyset-paginated list endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Organization to scope the list to.
    pub org_id: Uuid,
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

// --- handlers -----------------------------------------------------------------------

async fn create_rule(
    State(state): State<AppState>,
    Json(req): Json<CreateRuleRequest>,
) -> Result<(StatusCode, Json<ApiResponse<RuleDto>>), ApiFailure> {
    let kind = parse_field::<RuleKind>(&req.kind)?;
    let action = parse_field::<RuleAction>(&req.action)?;
    let rule = service(&state)
        .create_rule(OrgId::from_uuid(req.org_id), kind, &req.pattern, action)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(RuleDto::from(rule))),
    ))
}

async fn list_rules(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<RuleDto>>>, ApiFailure> {
    let rules = service(&state)
        .list_rules(
            OrgId::from_uuid(params.org_id),
            params.cursor(),
            params.limit(),
        )
        .await?;
    let dtos: Vec<RuleDto> = rules.into_iter().map(RuleDto::from).collect();
    Ok(Json(paged(dtos, params.limit())))
}

async fn list_decisions(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<DecisionDto>>>, ApiFailure> {
    let decisions = service(&state)
        .list_decisions(
            OrgId::from_uuid(params.org_id),
            params.cursor(),
            params.limit(),
        )
        .await?;
    let dtos: Vec<DecisionDto> = decisions.into_iter().map(DecisionDto::from).collect();
    Ok(Json(paged(dtos, params.limit())))
}

async fn get_decision(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<DecisionDto>>, ApiFailure> {
    let decision = service(&state).get_decision(id).await?;
    Ok(Json(ApiResponse::ok(DecisionDto::from(decision))))
}

async fn file_appeal(
    State(state): State<AppState>,
    Json(req): Json<FileAppealRequest>,
) -> Result<(StatusCode, Json<ApiResponse<AppealDto>>), ApiFailure> {
    let appeal = service(&state)
        .file_appeal(req.decision_id, &req.reason)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(AppealDto::from(appeal))),
    ))
}

async fn resolve_appeal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ResolveAppealRequest>,
) -> Result<Json<ApiResponse<AppealDto>>, ApiFailure> {
    let to = parse_field::<AppealStatus>(&req.status)?;
    let appeal = service(&state).resolve_appeal(id, to).await?;
    Ok(Json(ApiResponse::ok(AppealDto::from(appeal))))
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

fn parse_field<T>(raw: &str) -> Result<T, ApiFailure>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.parse::<T>()
        .map_err(|e| ApiFailure(Error::Validation(e.to_string())))
}

/// Wraps a [`dsoc_core::Error`] so it renders as a uniform [`ApiResponse`] failure with
/// the right HTTP status. Never leaks internal detail (storage errors render generically).
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
        let body = ApiResponse::<()>::fail(self.0.code(), self.0.to_string());
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
        Error::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        Error::Dependency { .. } => StatusCode::BAD_GATEWAY,
        // `Error` is #[non_exhaustive]; any future variant is a server-side concern.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let storage = Error::Storage(Box::new(std::io::Error::other("secret")));
        assert_eq!(status_for(&storage), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn rule_dto_maps_enum_tokens() {
        let r = Rule {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            kind: RuleKind::CapsRatio,
            pattern: "0.8".to_owned(),
            action: RuleAction::Reject,
            created_at: Utc::now(),
        };
        let dto = RuleDto::from(r);
        assert_eq!(dto.kind, "caps_ratio");
        assert_eq!(dto.action, "reject");
    }

    #[test]
    fn parse_field_rejects_unknown_token() {
        let err = parse_field::<RuleKind>("nonsense").unwrap_err();
        assert_eq!(status_for(&err.0), StatusCode::UNPROCESSABLE_ENTITY);
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
            limit: None,
            cursor_at: Some(Utc::now()),
            cursor_id: None,
        };
        assert!(none.cursor().is_none());
        assert_eq!(none.limit(), DEFAULT_PAGE_LIMIT);

        let both = ListParams {
            org_id: Uuid::now_v7(),
            limit: Some(10),
            cursor_at: Some(Utc::now()),
            cursor_id: Some(Uuid::now_v7()),
        };
        assert!(both.cursor().is_some());
        assert_eq!(both.limit(), 10);
    }
}
