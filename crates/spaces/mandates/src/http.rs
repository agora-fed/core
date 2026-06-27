//! Axum HTTP surface for `dsoc-mandates`. Exposes `pub fn routes(state: AppState) -> Router<()>`
//! (ADR-0004 wiring); it never binds a socket — the gateway owns the IPv6 bind. Handlers map the
//! lifecycle domain onto JSON DTOs wrapped in the shared [`ApiResponse`] envelope, and map
//! `dsoc_core::Error` to an HTTP status without leaking internal detail (SECURITY.md).
//!
//! AUTHORIZATION (ADR-0007): every mutating handler that an operator initiates (invite, bind
//! identity, add office) resolves the AUTHENTICATED caller from the [`dsoc_app::CallerId`] extractor
//! — the verified identity the gateway sets via the trusted `x-dsoc-citizen-id` / `x-dsoc-org-id`
//! headers (which the public ingress strips). It authorizes that caller via the injected
//! `Arc<dyn Authorization>` inside the service and NEVER trusts a citizen/org/mandate id taken from
//! the request body. Acceptance is the one mutation authenticated by the bearer of the one-time
//! invite **token** itself (the credential), so it carries no caller identity.

use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use dsoc_api_contract::envelope::ApiResponse;
use dsoc_api_contract::MandateDto;
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::{MandateId, OrgId};
use dsoc_core::Error;

use crate::domain::{self, OnboardingStatus};
use crate::service::{
    IdentityBinding, Invitation, MandateRegistry, MandateView, Office, OfficeDraft, Onboarding,
};

// ---------------------------------------------------------------------------
// Request / response DTOs (this crate owns its shapes; the gateway composes /openapi.json).
// ---------------------------------------------------------------------------

/// Query parameters carrying the organization/tenant for an operation.
#[derive(Debug, Clone, Deserialize)]
pub struct OrgQuery {
    /// The organization the mandate belongs to.
    pub org_id: Uuid,
}

/// Keyset pagination query parameters (with the org).
///
/// Two cursor shapes are exposed because the two list endpoints key differently: offices order by
/// id and use the single `after` id cursor; identity bindings order by `(verified_at, id)` and use
/// the composite `(after_at, after_id)` cursor. A composite cursor is honored only when BOTH
/// components are present (otherwise the first page is returned).
#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    /// The organization the mandate belongs to.
    pub org_id: Uuid,
    /// Return records strictly after this id (single-column keyset cursor, used by offices).
    pub after: Option<Uuid>,
    /// Composite keyset cursor — timestamp component (used by the identity-binding history).
    pub after_at: Option<DateTime<Utc>>,
    /// Composite keyset cursor — id tie-breaker component (used by the identity-binding history).
    pub after_id: Option<Uuid>,
    /// Page size (clamped to the service maximum).
    pub limit: Option<u32>,
}

/// Response to a successful invite: the one-time plaintext token to deliver to the official's
/// public email. SECURITY: this token is shown exactly once; the platform stores only its hash.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InvitationDto {
    /// Real stored invitation id.
    pub id: Uuid,
    /// The mandate invited.
    pub mandate_id: Uuid,
    /// The public email the invite was addressed to.
    pub public_email: String,
    /// The one-time plaintext invite token (deliver, then discard; never persisted).
    pub token: String,
    /// When the invite was sent.
    pub sent_at: DateTime<Utc>,
}

impl From<Invitation> for InvitationDto {
    fn from(value: Invitation) -> Self {
        Self {
            id: value.id,
            mandate_id: value.mandate.as_uuid(),
            public_email: value.public_email,
            token: value.token,
            sent_at: value.sent_at,
        }
    }
}

/// `POST /mandates/invitations/accept` body: the plaintext invite token to redeem.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AcceptInvitationRequest {
    /// The one-time invite token delivered to the official.
    pub token: String,
}

/// Response to a successful onboarding.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OnboardingDto {
    /// The mandate now onboarded.
    pub mandate_id: Uuid,
    /// When onboarding completed.
    pub onboarded_at: DateTime<Utc>,
}

impl From<Onboarding> for OnboardingDto {
    fn from(value: Onboarding) -> Self {
        Self {
            mandate_id: value.mandate.as_uuid(),
            onboarded_at: value.onboarded_at,
        }
    }
}

/// `POST /mandates/{id}/identity` body: record an identity-assurance binding.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct BindIdentityRequest {
    /// Assurance level (`anonymous` | `email` | `directory` | `strong`).
    pub level: String,
    /// Optional reference to the evidence (e.g. a TSE record id).
    pub evidence_ref: Option<String>,
}

/// Public view of an identity binding.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IdentityBindingDto {
    /// Binding id.
    pub id: Uuid,
    /// The mandate bound.
    pub mandate_id: Uuid,
    /// The assurance level recorded.
    pub verification_level: String,
    /// Reference to the evidence, if any.
    pub evidence_ref: Option<String>,
    /// When the binding was recorded.
    pub verified_at: DateTime<Utc>,
}

impl From<IdentityBinding> for IdentityBindingDto {
    fn from(value: IdentityBinding) -> Self {
        Self {
            id: value.id,
            mandate_id: value.mandate.as_uuid(),
            verification_level: domain::level_as_str(value.level).to_owned(),
            evidence_ref: value.evidence_ref,
            verified_at: value.verified_at,
        }
    }
}

/// `POST /mandates/{id}/offices` body: add a term-bound office record.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddOfficeRequest {
    /// Office held or sought (e.g. `vereador`).
    pub office: String,
    /// Electoral district / municipality, if any.
    pub district: Option<String>,
    /// Term start date (YYYY-MM-DD), if known.
    pub term_start: Option<NaiveDate>,
    /// Term end date (YYYY-MM-DD), if known.
    pub term_end: Option<NaiveDate>,
}

/// Public view of a term-bound office record.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OfficeDto {
    /// Office record id.
    pub id: Uuid,
    /// The mandate this office belongs to.
    pub mandate_id: Uuid,
    /// Office held or sought.
    pub office: String,
    /// Electoral district / municipality, if any.
    pub district: Option<String>,
    /// Term start date, if known.
    pub term_start: Option<NaiveDate>,
    /// Term end date, if known.
    pub term_end: Option<NaiveDate>,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
}

impl From<Office> for OfficeDto {
    fn from(value: Office) -> Self {
        Self {
            id: value.id,
            mandate_id: value.mandate.as_uuid(),
            office: value.office,
            district: value.district,
            term_start: value.term_start,
            term_end: value.term_end,
            created_at: value.created_at,
        }
    }
}

/// Map the registry's `MandateView` onto the shared `api-contract` `MandateDto` (the public
/// contract shape clients consume). The derived onboarding status collapses to the `onboarded`
/// boolean the contract exposes. Avatar URL is composed from `MEDIA_BASE_URL` (or `/media` for
/// the same-origin default) + the stored object key.
fn to_mandate_dto(view: MandateView) -> MandateDto {
    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let media_base = media_base.trim_end_matches('/');
    let avatar_url = view
        .avatar_object_key
        .map(|k| format!("{media_base}/{k}"));
    MandateDto {
        id: view.id.as_uuid(),
        office: view.office,
        display_name: view.display_name,
        is_candidate: view.is_candidate,
        onboarded: matches!(view.status, OnboardingStatus::Onboarded),
        party: view.party,
        uf: view.uf,
        house: view.house,
        avatar_url,
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the mandate-registry routes onto a router carrying the shared [`AppState`].
///
/// Reads are open; operator mutations require a verified actor (`x-citizen-id` + authorization).
/// Invite acceptance is authenticated by the bearer of the one-time token, not an actor header.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/mandates", get(list_mandates))
        .route("/mandates/invitations/accept", post(accept_invitation))
        .route("/mandates/{mandate_id}", get(get_mandate))
        .route("/mandates/{mandate_id}/invitations", post(invite))
        .route(
            "/mandates/{mandate_id}/identity",
            post(bind_identity).get(list_identity_bindings),
        )
        .route(
            "/mandates/{mandate_id}/offices",
            post(add_office).get(list_offices),
        )
        .with_state(state)
}

async fn invite(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    caller: CallerId,
) -> Result<(StatusCode, Json<ApiResponse<InvitationDto>>), ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let invitation = svc
        .invite(caller.org, caller.citizen, MandateId::from_uuid(mandate_id))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(InvitationDto::from(invitation))),
    ))
}

async fn accept_invitation(
    State(state): State<AppState>,
    Query(query): Query<OrgQuery>,
    Json(body): Json<AcceptInvitationRequest>,
) -> Result<Json<ApiResponse<OnboardingDto>>, ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let onboarding = svc
        .accept_invitation(OrgId::from_uuid(query.org_id), &body.token)
        .await?;
    Ok(Json(ApiResponse::ok(OnboardingDto::from(onboarding))))
}

async fn bind_identity(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    caller: CallerId,
    Json(body): Json<BindIdentityRequest>,
) -> Result<(StatusCode, Json<ApiResponse<IdentityBindingDto>>), ApiErr> {
    let level = domain::level_from_str(&body.level)?;
    let svc = MandateRegistry::from_state(&state);
    let binding = svc
        .bind_identity(
            caller.org,
            caller.citizen,
            MandateId::from_uuid(mandate_id),
            level,
            body.evidence_ref.as_deref(),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(IdentityBindingDto::from(binding))),
    ))
}

async fn add_office(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    caller: CallerId,
    Json(body): Json<AddOfficeRequest>,
) -> Result<(StatusCode, Json<ApiResponse<OfficeDto>>), ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let office = svc
        .add_office(
            caller.org,
            caller.citizen,
            MandateId::from_uuid(mandate_id),
            OfficeDraft {
                office: body.office,
                district: body.district,
                term_start: body.term_start,
                term_end: body.term_end,
            },
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(OfficeDto::from(office))),
    ))
}

async fn get_mandate(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> Result<Json<ApiResponse<MandateDto>>, ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let view = svc
        .get_mandate(
            OrgId::from_uuid(query.org_id),
            MandateId::from_uuid(mandate_id),
        )
        .await?;
    Ok(Json(ApiResponse::ok(to_mandate_dto(view))))
}

/// Query parameters for `GET /mandates`: the org and an optional offset/limit window.
#[derive(Debug, Clone, Deserialize)]
struct MandateListQuery {
    org_id: Uuid,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// `GET /mandates?org_id=&limit=&offset=` — directory of mandates in an org, ordered by display
/// name. Public; used by the front-end picker so people don't have to type a UUID by hand.
async fn list_mandates(
    State(state): State<AppState>,
    Query(query): Query<MandateListQuery>,
) -> Result<Json<ApiResponse<Vec<MandateDto>>>, ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let views = svc
        .list_mandates(OrgId::from_uuid(query.org_id), query.limit, query.offset)
        .await?;
    Ok(Json(ApiResponse::ok(
        views.into_iter().map(to_mandate_dto).collect(),
    )))
}

async fn list_offices(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<Vec<OfficeDto>>>, ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    let offices = svc
        .list_offices(MandateId::from_uuid(mandate_id), query.after, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        offices.into_iter().map(OfficeDto::from).collect(),
    )))
}

async fn list_identity_bindings(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<Vec<IdentityBindingDto>>>, ApiErr> {
    let svc = MandateRegistry::from_state(&state);
    // The binding history keysets on `(verified_at, id)`; honor the composite cursor only when BOTH
    // components are supplied, otherwise serve the first page. This lets callers paginate past the
    // first page (the prior surface dropped the cursor and was stuck on page 1).
    let cursor = match (query.after_at, query.after_id) {
        (Some(after_at), Some(after_id)) => Some((after_at, after_id)),
        _ => None,
    };
    let bindings = svc
        .list_identity_bindings(MandateId::from_uuid(mandate_id), cursor, query.limit)
        .await?;
    Ok(Json(ApiResponse::ok(
        bindings.into_iter().map(IdentityBindingDto::from).collect(),
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
            tracing::error!(code = self.0.code(), detail = %self.0, "mandates request failed");
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
        Error::NotFound(_) => "Mandato ou convite não encontrado.",
        Error::Forbidden(_) => "Acesso negado: verificação insuficiente para esta ação.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Dados inválidos na requisição.",
        Error::Conflict(_) => "Conflito de estado: o mandato já foi integrado.",
        // Storage, Dependency, and any future non-exhaustive variant: generic internal message.
        _ => "Erro interno ao processar o registro de mandatos.",
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
    fn mandate_dto_maps_onboarded_from_status() {
        let mk = |status| MandateView {
            id: MandateId::new(),
            office: "vereador".to_owned(),
            display_name: "Fulana".to_owned(),
            is_candidate: false,
            status,
            public_handle: "m-abc".to_owned(),
            party: None,
            uf: None,
            house: None,
            avatar_object_key: None,
        };
        assert!(to_mandate_dto(mk(OnboardingStatus::Onboarded)).onboarded);
        assert!(!to_mandate_dto(mk(OnboardingStatus::Invited)).onboarded);
        assert!(!to_mandate_dto(mk(OnboardingStatus::NotInvited)).onboarded);
    }

    #[test]
    fn invitation_dto_carries_one_time_token() {
        let dto = InvitationDto::from(Invitation {
            id: Uuid::now_v7(),
            mandate: MandateId::new(),
            public_email: "vereador@example.test".to_owned(),
            token: "deadbeef".to_owned(),
            sent_at: Utc::now(),
        });
        assert_eq!(dto.token, "deadbeef");
    }
}
