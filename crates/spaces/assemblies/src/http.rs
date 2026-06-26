//! Axum HTTP surface for `dsoc-assemblies`. Exposes `pub fn routes(state: AppState) -> Router<()>`
//! (ADR-0004 wiring); it never binds a socket — the gateway owns the IPv6 bind. Handlers map the
//! membership domain onto JSON DTOs wrapped in the shared [`ApiResponse`] envelope, and map
//! `dsoc_core::Error` to an HTTP status without leaking internal detail (SECURITY.md).
//!
//! AUTHORIZATION (ADR-0007): every mutating handler (create an assembly, add a member) resolves the
//! AUTHENTICATED caller from the [`dsoc_app::CallerId`] extractor — the verified identity the
//! gateway sets via the trusted `x-dsoc-citizen-id` / `x-dsoc-org-id` headers (which the public
//! ingress strips). It authorizes that caller via the injected `Arc<dyn Authorization>` inside the
//! service and NEVER trusts the acting citizen/org taken from the request body. The member being
//! ADDED is request data (a `citizen_id` in the body); the ACTING organizer never is.

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
use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::Error;

use crate::service::{AssembliesService, Assembly, Member};

// ---------------------------------------------------------------------------
// Request / response DTOs (this crate owns its shapes; the gateway composes /openapi.json).
// ---------------------------------------------------------------------------

/// Query parameters carrying the organization/tenant for a read operation.
#[derive(Debug, Clone, Deserialize)]
pub struct OrgQuery {
    /// The organization the assembly belongs to.
    pub org_id: Uuid,
}

/// Keyset pagination query parameters for the roster list (ordered by member id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    /// Return members strictly after this id (single-column keyset cursor).
    pub after: Option<Uuid>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

/// `POST /assemblies` body: create a permanent participatory body. The acting organizer and org
/// come from the authenticated caller, never this body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateAssemblyRequest {
    /// Public display name of the body.
    pub name: String,
}

/// Public view of an assembly.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssemblyDto {
    /// Assembly id.
    pub id: Uuid,
    /// The organization that owns the body.
    pub org_id: Uuid,
    /// Public display name.
    pub name: String,
    /// When the body was created.
    pub created_at: DateTime<Utc>,
}

impl From<Assembly> for AssemblyDto {
    fn from(value: Assembly) -> Self {
        Self {
            id: value.id.as_uuid(),
            org_id: value.org.as_uuid(),
            name: value.name,
            created_at: value.created_at,
        }
    }
}

/// `POST /assemblies/{id}/members` body: add a citizen to the roster. The acting organizer comes
/// from the authenticated caller; the `citizen_id` here is the member being added (request data).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    /// The citizen to add to the roster.
    pub citizen_id: Uuid,
    /// The member's role (`member` | `chair` | `secretary` | `observer`). Defaults to `member`.
    #[serde(default)]
    pub role: Option<String>,
}

/// Public view of a roster membership.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MemberDto {
    /// Membership id.
    pub id: Uuid,
    /// The assembly the citizen belongs to.
    pub assembly_id: Uuid,
    /// The member citizen.
    pub citizen_id: Uuid,
    /// The member's role.
    pub role: String,
    /// When the citizen joined.
    pub joined_at: DateTime<Utc>,
}

impl From<Member> for MemberDto {
    fn from(value: Member) -> Self {
        Self {
            id: value.id,
            assembly_id: value.assembly.as_uuid(),
            citizen_id: value.citizen.as_uuid(),
            role: value.role,
            joined_at: value.joined_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the assembly-membership routes onto a router carrying the shared [`AppState`].
///
/// Reads are open; organizer mutations require a verified caller (`x-dsoc-citizen-id` +
/// authorization). No cross-crate events are emitted (membership is internal state).
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/assemblies", post(create_assembly))
        .route("/assemblies/{assembly_id}", get(get_assembly))
        .route(
            "/assemblies/{assembly_id}/members",
            post(add_member).get(list_members),
        )
        .with_state(state)
}

async fn create_assembly(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<CreateAssemblyRequest>,
) -> Result<(StatusCode, Json<ApiResponse<AssemblyDto>>), ApiErr> {
    let svc = AssembliesService::from_state(&state);
    let assembly = svc
        .create_assembly(caller.org, caller.citizen, &body.name)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(AssemblyDto::from(assembly))),
    ))
}

async fn get_assembly(
    State(state): State<AppState>,
    Path(assembly_id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> Result<Json<ApiResponse<AssemblyDto>>, ApiErr> {
    let svc = AssembliesService::from_state(&state);
    let assembly = svc
        .get_assembly(
            OrgId::from_uuid(query.org_id),
            SpaceId::from_uuid(assembly_id),
        )
        .await?;
    Ok(Json(ApiResponse::ok(AssemblyDto::from(assembly))))
}

async fn add_member(
    State(state): State<AppState>,
    Path(assembly_id): Path<Uuid>,
    caller: CallerId,
    Json(body): Json<AddMemberRequest>,
) -> Result<(StatusCode, Json<ApiResponse<MemberDto>>), ApiErr> {
    let svc = AssembliesService::from_state(&state);
    let member = svc
        .add_member(
            caller.org,
            caller.citizen,
            SpaceId::from_uuid(assembly_id),
            CitizenId::from_uuid(body.citizen_id),
            body.role.as_deref().unwrap_or_default(),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(MemberDto::from(member))),
    ))
}

async fn list_members(
    State(state): State<AppState>,
    Path(assembly_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<Vec<MemberDto>>>, ApiErr> {
    let svc = AssembliesService::from_state(&state);
    let members = svc
        .list_members(SpaceId::from_uuid(assembly_id), query.after, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        members.into_iter().map(MemberDto::from).collect(),
    )))
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
            tracing::error!(code = self.0.code(), detail = %self.0, "assemblies request failed");
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
        Error::NotFound(_) => "Assembleia não encontrada.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Conflito de estado.",
        // Storage, Dependency, and any future non-exhaustive variant: generic internal message.
        _ => "Erro interno ao processar a assembleia.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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

    #[test]
    fn assembly_dto_maps_fields() {
        let id = SpaceId::new();
        let org = OrgId::new();
        let dto = AssemblyDto::from(Assembly {
            id,
            org,
            name: "Assembleia".to_owned(),
            created_at: Utc::now(),
        });
        assert_eq!(dto.id, id.as_uuid());
        assert_eq!(dto.org_id, org.as_uuid());
        assert_eq!(dto.name, "Assembleia");
    }

    #[test]
    fn member_dto_maps_fields() {
        let assembly = SpaceId::new();
        let citizen = CitizenId::new();
        let mid = Uuid::now_v7();
        let dto = MemberDto::from(Member {
            id: mid,
            assembly,
            citizen,
            role: "chair".to_owned(),
            joined_at: Utc::now(),
        });
        assert_eq!(dto.id, mid);
        assert_eq!(dto.assembly_id, assembly.as_uuid());
        assert_eq!(dto.citizen_id, citizen.as_uuid());
        assert_eq!(dto.role, "chair");
    }
}
