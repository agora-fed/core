//! Axum HTTP surface for `dsoc-admin`. Handlers map the domain onto JSON DTOs wrapped in
//! the shared [`ApiResponse`] envelope (`dsoc-api-contract`). This module **never binds a
//! socket** — the gateway owns the IPv6 bind (ADR-0004). Mutations require a high
//! verification level, enforced by [`AdminService`]; reads are open.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::envelope::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::Error;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{AdminOrg, AdminRole, FeatureFlag, RoleBinding};
use crate::service::AdminService;

/// HTTP header carrying the acting citizen's id (injected by the gateway's auth layer).
const ACTOR_HEADER: &str = "x-citizen-id";

/// Public view of an administrative org.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AdminOrgDto {
    /// Baseline organization id.
    pub org_id: Uuid,
    /// Whether the org is administratively active.
    pub is_active: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<AdminOrg> for AdminOrgDto {
    fn from(value: AdminOrg) -> Self {
        Self {
            org_id: value.org_id.as_uuid(),
            is_active: value.is_active,
            created_at: value.created_at,
        }
    }
}

/// Public view of a role binding.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RoleBindingDto {
    /// Binding id.
    pub id: Uuid,
    /// Organization id.
    pub org_id: Uuid,
    /// Citizen granted the role.
    pub citizen_id: Uuid,
    /// Role name (`owner` | `admin` | `auditor`).
    pub role: String,
    /// When the grant was made.
    pub created_at: DateTime<Utc>,
}

impl From<RoleBinding> for RoleBindingDto {
    fn from(value: RoleBinding) -> Self {
        Self {
            id: value.id,
            org_id: value.org_id.as_uuid(),
            citizen_id: value.citizen_id.as_uuid(),
            role: value.role.as_str().to_owned(),
            created_at: value.created_at,
        }
    }
}

/// Public view of a feature flag.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FeatureFlagDto {
    /// Flag id.
    pub id: Uuid,
    /// Organization id.
    pub org_id: Uuid,
    /// Flag key.
    pub key: String,
    /// Whether the feature is enabled.
    pub enabled: bool,
    /// When the flag was created.
    pub created_at: DateTime<Utc>,
    /// When the flag was last changed.
    pub updated_at: DateTime<Utc>,
}

impl From<FeatureFlag> for FeatureFlagDto {
    fn from(value: FeatureFlag) -> Self {
        Self {
            id: value.id,
            org_id: value.org_id.as_uuid(),
            key: value.key,
            enabled: value.enabled,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// Request body to create (link) an administrative org.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateOrgRequest {
    /// The baseline organization to administer.
    pub org_id: Uuid,
}

/// Request body to bind a role to a citizen.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct BindRoleRequest {
    /// Citizen to grant the role to.
    pub citizen_id: Uuid,
    /// Role name (`owner` | `admin` | `auditor`).
    pub role: String,
}

/// Request body to set a feature flag.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetFlagRequest {
    /// Desired enabled state.
    pub enabled: bool,
}

/// Keyset pagination query parameters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListParams {
    /// Return records strictly after this id (keyset cursor).
    pub after: Option<Uuid>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

/// Mount the admin routes onto a router carrying the shared [`AppState`].
///
/// Reads are open; mutations (`POST`/`PUT`) require a verified actor (see [`AdminService`]).
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/orgs", post(create_org).get(list_orgs))
        .route("/admin/orgs/{org_id}", get(get_org))
        .route(
            "/admin/orgs/{org_id}/roles",
            post(bind_role).get(list_role_bindings),
        )
        .route("/admin/orgs/{org_id}/flags", get(list_feature_flags))
        .route(
            "/admin/orgs/{org_id}/flags/{key}",
            get(get_feature_flag).put(set_feature_flag),
        )
        .with_state(state)
}

async fn create_org(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgRequest>,
) -> Result<(StatusCode, Json<ApiResponse<AdminOrgDto>>), ApiErr> {
    let actor = actor_from_headers(&headers)?;
    let svc = AdminService::from_state(&state);
    let created = svc.create_org(OrgId::from_uuid(body.org_id), actor).await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(AdminOrgDto::from(created))),
    ))
}

async fn get_org(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
) -> Result<Json<ApiResponse<AdminOrgDto>>, ApiErr> {
    let svc = AdminService::from_state(&state);
    let org = svc.get_org(OrgId::from_uuid(org_id)).await?;
    Ok(Json(ApiResponse::ok(AdminOrgDto::from(org))))
}

async fn list_orgs(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<AdminOrgDto>>>, ApiErr> {
    let svc = AdminService::from_state(&state);
    let orgs = svc
        .list_orgs(params.after.map(OrgId::from_uuid), params.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        orgs.into_iter().map(AdminOrgDto::from).collect(),
    )))
}

async fn bind_role(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<BindRoleRequest>,
) -> Result<(StatusCode, Json<ApiResponse<RoleBindingDto>>), ApiErr> {
    let actor = actor_from_headers(&headers)?;
    let role = AdminRole::parse(&body.role)?;
    let svc = AdminService::from_state(&state);
    let binding = svc
        .bind_role(
            OrgId::from_uuid(org_id),
            actor,
            CitizenId::from_uuid(body.citizen_id),
            role,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(RoleBindingDto::from(binding))),
    ))
}

async fn list_role_bindings(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<RoleBindingDto>>>, ApiErr> {
    let svc = AdminService::from_state(&state);
    let bindings = svc
        .list_role_bindings(OrgId::from_uuid(org_id), params.after, params.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        bindings.into_iter().map(RoleBindingDto::from).collect(),
    )))
}

async fn set_feature_flag(
    State(state): State<AppState>,
    Path((org_id, key)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(body): Json<SetFlagRequest>,
) -> Result<Json<ApiResponse<FeatureFlagDto>>, ApiErr> {
    let actor = actor_from_headers(&headers)?;
    let svc = AdminService::from_state(&state);
    let flag = svc
        .set_feature_flag(OrgId::from_uuid(org_id), actor, &key, body.enabled)
        .await?;
    Ok(Json(ApiResponse::ok(FeatureFlagDto::from(flag))))
}

async fn get_feature_flag(
    State(state): State<AppState>,
    Path((org_id, key)): Path<(Uuid, String)>,
) -> Result<Json<ApiResponse<FeatureFlagDto>>, ApiErr> {
    let svc = AdminService::from_state(&state);
    let flag = svc.get_feature_flag(OrgId::from_uuid(org_id), &key).await?;
    Ok(Json(ApiResponse::ok(FeatureFlagDto::from(flag))))
}

async fn list_feature_flags(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    Query(params): Query<ListParams>,
) -> Result<Json<ApiResponse<Vec<FeatureFlagDto>>>, ApiErr> {
    let svc = AdminService::from_state(&state);
    let flags = svc
        .list_feature_flags(OrgId::from_uuid(org_id), params.after, params.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        flags.into_iter().map(FeatureFlagDto::from).collect(),
    )))
}

/// Extract the acting citizen id from the request headers.
fn actor_from_headers(headers: &HeaderMap) -> Result<CitizenId, Error> {
    let raw = headers
        .get(ACTOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Unauthorized)?;
    let id = Uuid::parse_str(raw).map_err(|_| Error::Unauthorized)?;
    Ok(CitizenId::from_uuid(id))
}

/// Newtype adapting [`dsoc_core::Error`] into an HTTP response with the right status code and
/// a public-safe, Portuguese end-user message (never leaks internals; coding-style/security).
struct ApiErr(Error);

impl From<Error> for ApiErr {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Conflict(_) => StatusCode::CONFLICT,
            // Storage, Dependency, and any future non-exhaustive variant are internal.
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = ApiResponse::<AdminOrgDto>::fail(self.0.code(), message_pt(&self.0));
        (status, Json(body)).into_response()
    }
}

/// Map a canonical error to a Portuguese, public-safe end-user message.
fn message_pt(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Registro administrativo não encontrado.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Registro administrativo já existe.",
        // Storage, Dependency, and any future non-exhaustive variant: generic internal message.
        _ => "Erro interno ao processar a administração.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use axum::http::HeaderValue;
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn actor_header_parses_valid_uuid() {
        let id = Uuid::now_v7();
        let mut headers = HeaderMap::new();
        headers.insert(
            ACTOR_HEADER,
            HeaderValue::from_str(&id.to_string()).unwrap(),
        );
        let actor = actor_from_headers(&headers).unwrap();
        assert_eq!(actor.as_uuid(), id);
    }

    #[test]
    fn actor_header_missing_is_unauthorized() {
        let err = actor_from_headers(&HeaderMap::new()).unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn actor_header_invalid_is_unauthorized() {
        let mut headers = HeaderMap::new();
        headers.insert(ACTOR_HEADER, HeaderValue::from_static("not-a-uuid"));
        let err = actor_from_headers(&headers).unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn org_dto_maps_from_domain() {
        let org = OrgId::new();
        let dto = AdminOrgDto::from(AdminOrg {
            org_id: org,
            is_active: true,
            created_at: ts(),
        });
        assert_eq!(dto.org_id, org.as_uuid());
        assert!(dto.is_active);
    }

    #[test]
    fn role_binding_dto_renders_role_string() {
        let dto = RoleBindingDto::from(RoleBinding {
            id: Uuid::now_v7(),
            org_id: OrgId::new(),
            citizen_id: CitizenId::new(),
            role: AdminRole::Auditor,
            created_at: ts(),
        });
        assert_eq!(dto.role, "auditor");
    }

    #[test]
    fn feature_flag_dto_maps_from_domain() {
        let dto = FeatureFlagDto::from(FeatureFlag {
            id: Uuid::now_v7(),
            org_id: OrgId::new(),
            key: "proposals.clustering".to_owned(),
            enabled: true,
            created_at: ts(),
            updated_at: ts(),
        });
        assert_eq!(dto.key, "proposals.clustering");
        assert!(dto.enabled);
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
            assert_eq!(ApiErr::from(err).into_response().status(), expected);
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
