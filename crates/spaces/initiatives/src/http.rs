//! Axum HTTP surface for `dsoc-initiatives`. Exposes `pub fn routes(state: AppState) -> Router<()>`
//! (ADR-0004 wiring); it never binds a socket — the gateway owns the IPv6 bind. Handlers map the
//! domain onto JSON DTOs wrapped in the shared [`ApiResponse`] envelope, and map `dsoc_core::Error`
//! to an HTTP status without leaking internal detail (SECURITY.md).
//!
//! AUTHORIZATION (ADR-0007): every mutating handler (create, sign) resolves the AUTHENTICATED
//! caller from the [`dsoc_app::CallerId`] extractor — the verified identity the gateway sets via the
//! trusted `x-dsoc-citizen-id` / `x-dsoc-org-id` headers (which the public ingress strips). It
//! authorizes that caller via the injected `Arc<dyn Authorization>` inside the service at the Email
//! level and NEVER trusts a citizen/org id taken from the request body. Reads are open and take the
//! organization from a query parameter.

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

use crate::service::{Initiative, InitiativeService, SignOutcome};

// ---------------------------------------------------------------------------
// Request / response DTOs (this crate owns its shapes; the gateway composes /openapi.json).
// ---------------------------------------------------------------------------

/// Query parameter carrying the organization/tenant for a read.
#[derive(Debug, Clone, Deserialize)]
pub struct OrgQuery {
    /// The organization the initiative belongs to.
    pub org_id: Uuid,
}

/// Keyset pagination query parameters (with the org).
#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    /// The organization whose initiatives to list.
    pub org_id: Uuid,
    /// Return records strictly after this id (keyset cursor).
    pub after: Option<Uuid>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

/// `POST /initiatives` body: create a citizen initiative.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateInitiativeRequest {
    /// Human title of the initiative.
    pub title: String,
    /// Full body text.
    pub body: String,
    /// Signatures required to escalate (>= 1).
    pub threshold: i32,
}

/// Public view of an initiative.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InitiativeDto {
    /// Initiative id.
    pub id: Uuid,
    /// Organization id.
    pub org_id: Uuid,
    /// Human title.
    pub title: String,
    /// Full body text.
    pub body: String,
    /// Signatures required to escalate.
    pub threshold: i32,
    /// Current running signature tally.
    pub signature_count: i64,
    /// Whether the initiative has crossed its threshold (escalated).
    pub reached: bool,
    /// When the initiative reached its threshold, if it has.
    pub reached_at: Option<DateTime<Utc>>,
    /// When the initiative was created.
    pub created_at: DateTime<Utc>,
}

impl From<Initiative> for InitiativeDto {
    fn from(value: Initiative) -> Self {
        Self {
            id: value.id,
            org_id: value.org.as_uuid(),
            title: value.title,
            body: value.body,
            threshold: value.threshold,
            signature_count: value.signature_count,
            reached: value.reached_at.is_some(),
            reached_at: value.reached_at,
            created_at: value.created_at,
        }
    }
}

/// Public view of a sign outcome.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SignOutcomeDto {
    /// The stored signature id.
    pub signature_id: Uuid,
    /// The initiative signed.
    pub initiative_id: Uuid,
    /// The citizen who signed.
    pub citizen_id: Uuid,
    /// The signature tally after this sign.
    pub signature_count: i64,
    /// The threshold required to escalate.
    pub threshold: i32,
    /// Whether the initiative has crossed its threshold (escalated).
    pub reached: bool,
    /// When the initiative reached its threshold, if it has.
    pub reached_at: Option<DateTime<Utc>>,
    /// Whether THIS request recorded a new signature (`false` on an idempotent repeat sign).
    pub newly_signed: bool,
}

impl From<SignOutcome> for SignOutcomeDto {
    fn from(value: SignOutcome) -> Self {
        Self {
            signature_id: value.signature_id,
            initiative_id: value.initiative_id,
            citizen_id: value.citizen.as_uuid(),
            signature_count: value.signature_count,
            threshold: value.threshold,
            reached: value.reached_at.is_some(),
            reached_at: value.reached_at,
            newly_signed: value.newly_signed,
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the initiatives routes onto a router carrying the shared [`AppState`].
///
/// Reads are open (org via query); mutations (create, sign) require a verified caller authorized at
/// the Email level (ADR-0007), taken from the [`CallerId`] extractor — never the body.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/initiatives",
            post(create_initiative).get(list_initiatives),
        )
        .route("/initiatives/{initiative_id}", get(get_initiative))
        .route(
            "/initiatives/{initiative_id}/signatures",
            post(sign_initiative),
        )
        .with_state(state)
}

async fn create_initiative(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<CreateInitiativeRequest>,
) -> Result<(StatusCode, Json<ApiResponse<InitiativeDto>>), ApiErr> {
    let svc = InitiativeService::from_state(&state);
    let initiative = svc
        .create(
            caller.org,
            caller.citizen,
            &body.title,
            &body.body,
            body.threshold,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(InitiativeDto::from(initiative))),
    ))
}

async fn sign_initiative(
    State(state): State<AppState>,
    Path(initiative_id): Path<Uuid>,
    caller: CallerId,
) -> Result<(StatusCode, Json<ApiResponse<SignOutcomeDto>>), ApiErr> {
    let svc = InitiativeService::from_state(&state);
    let outcome = svc.sign(caller.org, caller.citizen, initiative_id).await?;
    // 201 when a new signature was recorded; 200 on an idempotent repeat sign.
    let status = if outcome.newly_signed {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(ApiResponse::ok(SignOutcomeDto::from(outcome)))))
}

async fn get_initiative(
    State(state): State<AppState>,
    Path(initiative_id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> Result<Json<ApiResponse<InitiativeDto>>, ApiErr> {
    let svc = InitiativeService::from_state(&state);
    let initiative = svc
        .get(OrgId::from_uuid(query.org_id), initiative_id)
        .await?;
    Ok(Json(ApiResponse::ok(InitiativeDto::from(initiative))))
}

async fn list_initiatives(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<Vec<InitiativeDto>>>, ApiErr> {
    let svc = InitiativeService::from_state(&state);
    let initiatives = svc
        .list(OrgId::from_uuid(query.org_id), query.after, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        initiatives.into_iter().map(InitiativeDto::from).collect(),
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
            tracing::error!(code = self.0.code(), detail = %self.0, "initiatives request failed");
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
        Error::NotFound(_) => "Iniciativa não encontrada.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Conflito de estado na iniciativa.",
        // Storage, Dependency, and any future non-exhaustive variant: generic internal message.
        _ => "Erro interno ao processar a iniciativa.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use dsoc_core::ids::CitizenId;

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
    fn initiative_dto_maps_reached_from_marker() {
        let now = Utc::now();
        let reached = InitiativeDto::from(Initiative {
            id: Uuid::now_v7(),
            org: OrgId::new(),
            title: "t".to_owned(),
            body: "b".to_owned(),
            threshold: 3,
            signature_count: 3,
            reached_at: Some(now),
            created_at: now,
        });
        assert!(reached.reached);
        assert_eq!(reached.reached_at, Some(now));

        let pending = InitiativeDto::from(Initiative {
            id: Uuid::now_v7(),
            org: OrgId::new(),
            title: "t".to_owned(),
            body: "b".to_owned(),
            threshold: 3,
            signature_count: 1,
            reached_at: None,
            created_at: now,
        });
        assert!(!pending.reached);
    }

    #[test]
    fn sign_outcome_dto_carries_tally_and_flags() {
        let dto = SignOutcomeDto::from(SignOutcome {
            signature_id: Uuid::now_v7(),
            initiative_id: Uuid::now_v7(),
            citizen: CitizenId::new(),
            signature_count: 5,
            threshold: 3,
            reached_at: Some(Utc::now()),
            newly_signed: true,
        });
        assert_eq!(dto.signature_count, 5);
        assert!(dto.reached);
        assert!(dto.newly_signed);
    }
}
