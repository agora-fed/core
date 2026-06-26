//! Axum HTTP surface. Exposes `pub fn routes(state: AppState) -> Router<()>` (ADR-0004 wiring);
//! it never binds a socket — the gateway owns the IPv6 bind. Domain results are mapped to
//! `api-contract` DTOs wrapped in the uniform `ApiResponse` envelope, and `dsoc_core::Error` is
//! mapped to HTTP status without leaking internal detail (SECURITY.md).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use axum::extract::{Json, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde::Deserialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, Result};

use crate::domain::{KeySource, TokenValidator, DEFAULT_SESSION_TTL_SECS, IDENTITY_DEPENDENCY};
use crate::dto::{CreateSessionRequest, MeDto, SessionDto};
use crate::service::ZitadelAuth;

/// Build the routed service surface. Reads sovereign-issuer configuration from the environment
/// (never hardcoded — PLAN.md principle 8). When the issuer/JWKS is unconfigured the endpoints
/// reject with a dependency error rather than panicking.
pub fn routes(state: AppState) -> Router<()> {
    let svc = build_service(&state);
    Router::new()
        .route("/auth/session", post(create_session))
        .route("/auth/me", get(me))
        .with_state(svc)
}

fn build_service(state: &AppState) -> Arc<ZitadelAuth> {
    Arc::new(build_zitadel(
        state.db.clone(),
        state.clock.clone(),
        state.bus.clone(),
    ))
}

/// Build the OIDC token validator from the sovereign-issuer environment (never hardcoded —
/// PLAN.md principle 8). When unconfigured, returns a validator that rejects every token.
fn validator_from_env() -> TokenValidator {
    let issuer = std::env::var("AUTH_OIDC_ISSUER").ok();
    let jwks_url = std::env::var("AUTH_OIDC_JWKS_URL").ok();
    let audience = std::env::var("AUTH_OIDC_AUDIENCE").ok();
    match (issuer, jwks_url) {
        (Some(issuer), Some(jwks_url)) => {
            TokenValidator::new(Arc::new(JwksKeySource::new(jwks_url, issuer, audience)))
        }
        _ => {
            tracing::error!(
                "AUTH_OIDC_ISSUER / AUTH_OIDC_JWKS_URL are not configured; /auth endpoints will reject"
            );
            TokenValidator::new(Arc::new(UnconfiguredKeySource))
        }
    }
}

fn build_zitadel(
    db: dsoc_db::Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    bus: std::sync::Arc<dyn dsoc_core::EventBus>,
) -> ZitadelAuth {
    let session_ttl_secs = std::env::var("AUTH_SESSION_TTL_SECS")
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SESSION_TTL_SECS);
    ZitadelAuth::new(db, clock, bus, validator_from_env(), session_ttl_secs)
}

/// Construct the shared [`dsoc_core::Authorization`] port (for `AppState.authz`) from env config.
/// The gateway injects this so every crate can call `authz.require(...)`; the verification-level
/// checks only read the database, so they work regardless of OIDC reachability.
#[must_use]
pub fn authorization(
    db: dsoc_db::Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    bus: std::sync::Arc<dyn dsoc_core::EventBus>,
) -> std::sync::Arc<dyn dsoc_core::Authorization> {
    std::sync::Arc::new(build_zitadel(db, clock, bus))
}

/// Query parameters for `GET /auth/me`.
#[derive(Debug, Deserialize)]
struct MeQuery {
    org_id: Uuid,
}

async fn create_session(
    State(svc): State<Arc<ZitadelAuth>>,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    let org = OrgId::from_uuid(request.org_id);
    match svc.create_session(org, &request.token).await {
        Ok(session) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(SessionDto::from(session))),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

async fn me(
    State(svc): State<Arc<ZitadelAuth>>,
    Query(query): Query<MeQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return error_response(&Error::Unauthorized);
    };
    let org = OrgId::from_uuid(query.org_id);
    match svc.me(org, &token).await {
        Ok(identity) => {
            (StatusCode::OK, Json(ApiResponse::ok(MeDto::from(identity)))).into_response()
        }
        Err(error) => error_response(&error),
    }
}

/// Extract a bearer token from the `Authorization` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

/// Map a domain error to its HTTP status (stable, public-safe).
fn status_for(error: &Error) -> StatusCode {
    match error {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::Dependency { .. } => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// End-user message (Portuguese — civic content policy) that never leaks internal detail.
fn message_for(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Recurso não encontrado.",
        Error::Forbidden(_) => "Acesso negado.",
        Error::Unauthorized => "Não autenticado.",
        Error::Validation(_) => "Dados inválidos.",
        Error::Conflict(_) => "Conflito de estado.",
        Error::Dependency { .. } => "Falha ao contatar dependência soberana.",
        _ => "Erro interno do servidor.",
    }
}

fn error_response(error: &Error) -> Response {
    // Log internal detail server-side only; the body carries a stable code + safe message.
    if matches!(error, Error::Storage(_) | Error::Dependency { .. }) {
        tracing::error!(code = error.code(), detail = %error, "auth request failed");
    }
    let body = ApiResponse::<()>::fail(error.code(), message_for(error));
    (status_for(error), Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Production key sources (lower coverage; the domain `StaticKeySource` carries the tested path).
// ---------------------------------------------------------------------------

/// A [`KeySource`] resolving RS256 keys from a sovereign Zitadel JWKS endpoint, cached by `kid`.
pub(crate) struct JwksKeySource {
    http: reqwest::Client,
    jwks_url: String,
    issuer: String,
    audience: Option<String>,
    cache: Mutex<HashMap<String, DecodingKey>>,
}

impl fmt::Debug for JwksKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwksKeySource")
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

impl JwksKeySource {
    pub(crate) fn new(jwks_url: String, issuer: String, audience: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            jwks_url,
            issuer,
            audience,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn dependency(source: impl std::error::Error + Send + Sync + 'static) -> Error {
        Error::Dependency {
            dependency: IDENTITY_DEPENDENCY,
            source: Box::new(source),
        }
    }
}

#[async_trait::async_trait]
impl KeySource for JwksKeySource {
    async fn decoding_key(&self, kid: Option<&str>) -> Result<DecodingKey> {
        let cache_key = kid.unwrap_or_default().to_owned();
        if let Some(key) = self.cache.lock().await.get(&cache_key) {
            return Ok(key.clone());
        }

        let document: JwksDocument = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(Self::dependency)?
            .error_for_status()
            .map_err(Self::dependency)?
            .json()
            .await
            .map_err(Self::dependency)?;

        let single = document.keys.len() == 1;
        let mut cache = self.cache.lock().await;
        for jwk in &document.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            if let Ok(decoding) = DecodingKey::from_rsa_components(&jwk.n, &jwk.e) {
                let entry = jwk.kid.clone().unwrap_or_default();
                if single {
                    // Allow lookup by the (possibly absent) kid the caller presented.
                    cache.insert(String::new(), decoding.clone());
                }
                cache.insert(entry, decoding);
            }
        }
        cache.get(&cache_key).cloned().ok_or(Error::Unauthorized)
    }

    fn algorithm(&self) -> Algorithm {
        Algorithm::RS256
    }

    fn issuer(&self) -> &str {
        &self.issuer
    }

    fn expected_audience(&self) -> Option<&str> {
        self.audience.as_deref()
    }
}

/// Fallback used when no issuer/JWKS is configured: every validation fails as a dependency error.
struct UnconfiguredKeySource;

#[async_trait::async_trait]
impl KeySource for UnconfiguredKeySource {
    async fn decoding_key(&self, _kid: Option<&str>) -> Result<DecodingKey> {
        Err(Error::Dependency {
            dependency: IDENTITY_DEPENDENCY,
            source: "auth OIDC issuer/JWKS not configured".into(),
        })
    }

    fn algorithm(&self) -> Algorithm {
        Algorithm::RS256
    }

    fn issuer(&self) -> &str {
        ""
    }

    fn expected_audience(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn status_codes_map_each_error() {
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&Error::Storage("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for(&Error::Dependency {
                dependency: "zitadel",
                source: "x".into()
            }),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn messages_are_non_empty_and_safe() {
        // The storage message must not echo the internal detail string.
        let msg = message_for(&Error::Storage("secret-dsn".into()));
        assert!(!msg.contains("secret-dsn"));
        assert!(!message_for(&Error::Unauthorized).is_empty());
    }

    #[test]
    fn bearer_token_parses_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc.def.ghi".parse().unwrap());
        assert_eq!(bearer_token(&headers).as_deref(), Some("abc.def.ghi"));

        let mut bad = HeaderMap::new();
        bad.insert(header::AUTHORIZATION, "Basic xyz".parse().unwrap());
        assert!(bearer_token(&bad).is_none());

        assert!(bearer_token(&HeaderMap::new()).is_none());
    }
}
