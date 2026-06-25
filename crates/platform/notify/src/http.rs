//! Axum handlers for `dsoc-notify`, mounted by the gateway via [`routes`].
//!
//! Per ADR-0004 the crate exposes `routes(state: AppState) -> Router<()>` and never binds a
//! socket. Domain results map to `api-contract` envelopes; errors map to public-safe codes.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::error::{Error, Result};
use dsoc_core::ids::{CitizenId, OrgId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::service::NotifyService;

/// HTTP header carrying the authenticated caller's citizen id (injected by the gateway's auth
/// layer; never accepted from the request body).
const ACTOR_HEADER: &str = "x-citizen-id";

/// Request body for registering a push device token.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RegisterDeviceTokenRequest {
    /// Organization the caller is acting within (drives the verification check).
    pub org_id: Uuid,
    /// Citizen the token belongs to. Must equal the authenticated caller.
    pub citizen_id: Uuid,
    /// Push platform: `ios`, `android`, or `web`.
    pub platform: String,
    /// Provider-issued opaque device token.
    pub token: String,
}

/// Response body confirming a registered device token.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeviceTokenDto {
    /// Stored device-token id.
    pub id: Uuid,
    /// Citizen the token belongs to.
    pub citizen_id: Uuid,
    /// Push platform the token was registered for.
    pub platform: String,
}

/// Build the crate's router. The gateway supplies the [`AppState`] and owns the bind.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/notify/device-token", post(register_device_token))
        .with_state(state)
}

/// `POST /notify/device-token` — register a push device token for a citizen.
#[utoipa::path(
    post,
    path = "/notify/device-token",
    request_body = RegisterDeviceTokenRequest,
    responses(
        (status = 201, description = "Device token registered", body = DeviceTokenDto),
        (status = 400, description = "Invalid platform or token"),
        (status = 401, description = "Missing or malformed caller identity"),
        (status = 403, description = "Caller may not register a token for another citizen")
    ),
    tag = "notify"
)]
async fn register_device_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterDeviceTokenRequest>,
) -> Response {
    match register(&state, &headers, &req).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(DeviceTokenDto {
                id,
                citizen_id: req.citizen_id,
                platform: req.platform.clone(),
            })),
        )
            .into_response(),
        Err(err) => (
            status_for(&err),
            Json(ApiResponse::<DeviceTokenDto>::fail(
                err.code(),
                err.to_string(),
            )),
        )
            .into_response(),
    }
}

/// Authorize and perform a device-token registration.
///
/// The acting citizen is taken from the trusted [`ACTOR_HEADER`] (never the body) and must
/// match the `citizen_id` in the request — closing the notification-hijacking hole where a
/// caller could register a push token against someone else's account. The service layer then
/// enforces email verification via the injected [`dsoc_core::Authorization`] port.
async fn register(
    state: &AppState,
    headers: &HeaderMap,
    req: &RegisterDeviceTokenRequest,
) -> Result<Uuid> {
    let caller = actor_from_headers(headers)?;
    let citizen = CitizenId::from_uuid(req.citizen_id);
    ensure_self(caller, citizen)?;
    NotifyService::from_state(state)
        .register_device_token(
            OrgId::from_uuid(req.org_id),
            citizen,
            &req.platform,
            &req.token,
        )
        .await
}

/// Extract the authenticated caller's citizen id from the trusted gateway header.
fn actor_from_headers(headers: &HeaderMap) -> Result<CitizenId> {
    let raw = headers
        .get(ACTOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Unauthorized)?;
    let id = Uuid::parse_str(raw).map_err(|_| Error::Unauthorized)?;
    Ok(CitizenId::from_uuid(id))
}

/// Assert the caller is acting for themselves, not another citizen.
///
/// # Errors
/// [`Error::Forbidden`] when `caller` differs from `citizen`.
fn ensure_self(caller: CitizenId, citizen: CitizenId) -> Result<()> {
    if caller == citizen {
        Ok(())
    } else {
        Err(Error::Forbidden(
            "cannot register a device token for another citizen".into(),
        ))
    }
}

/// Map a domain [`Error`] to the HTTP status the gateway returns.
fn status_for(err: &Error) -> StatusCode {
    match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        // `Error` is `#[non_exhaustive]`; treat current and any future internal
        // failures as a server error rather than leaking detail.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn actor_header_parses_valid_uuid() {
        let id = Uuid::now_v7();
        let mut headers = HeaderMap::new();
        headers.insert(
            ACTOR_HEADER,
            HeaderValue::from_str(&id.to_string()).unwrap(),
        );
        let caller = actor_from_headers(&headers).unwrap();
        assert_eq!(caller, CitizenId::from_uuid(id));
    }

    #[test]
    fn actor_header_missing_is_unauthorized() {
        let err = actor_from_headers(&HeaderMap::new()).unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn actor_header_malformed_is_unauthorized() {
        let mut headers = HeaderMap::new();
        headers.insert(ACTOR_HEADER, HeaderValue::from_static("not-a-uuid"));
        let err = actor_from_headers(&headers).unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }

    #[test]
    fn ensure_self_allows_caller_acting_for_themselves() {
        let me = CitizenId::new();
        assert!(ensure_self(me, me).is_ok());
    }

    #[test]
    fn ensure_self_rejects_acting_for_another_citizen() {
        let caller = CitizenId::new();
        let victim = CitizenId::new();
        let err = ensure_self(caller, victim).unwrap_err();
        // A hijack attempt is forbidden, not merely unauthorized.
        assert_eq!(err.code(), "forbidden");
    }

    #[test]
    fn status_mapping_is_exhaustive_and_correct() {
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
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
        let storage = Error::Storage(Box::new(std::io::Error::other("boom")));
        assert_eq!(status_for(&storage), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn request_deserializes_from_json() {
        let body = serde_json::json!({
            "org_id": Uuid::now_v7(),
            "citizen_id": Uuid::now_v7(),
            "platform": "ios",
            "token": "abc",
        });
        let req: RegisterDeviceTokenRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.platform, "ios");
        assert_eq!(req.token, "abc");
    }
}
