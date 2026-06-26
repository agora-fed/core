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

use dsoc_api_contract::{ApiResponse, ProfileUpdateDto};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, Result};

use crate::domain::{KeySource, TokenValidator, DEFAULT_SESSION_TTL_SECS, IDENTITY_DEPENDENCY};
use crate::dto::{CreateSessionRequest, LoginRequest, MeDto, RegisterRequest, SessionDto};
use crate::password_reset::PasswordResetService;
use crate::profile::{ProfileService, ProfileUpdate};
use crate::service::{IssuedSession, ZitadelAuth};

/// Build the routed service surface. Reads sovereign-issuer configuration from the environment
/// (never hardcoded — PLAN.md principle 8). When the issuer/JWKS is unconfigured the endpoints
/// reject with a dependency error rather than panicking.
pub fn routes(state: AppState) -> Router<()> {
    let svc = build_service(&state);
    // Profile routes carry AppState (the CallerId extractor pulls identity from middleware-set
    // headers, and the ProfileService is built per-request from state — cheap, no shared mutable
    // surface). Auth routes carry Arc<ZitadelAuth>; the two sub-routers are merged.
    let auth_routes = Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/session", post(create_session))
        .route("/auth/me", get(me))
        .with_state(svc);
    let profile_routes = Router::new()
        .route("/me", get(get_me_profile).patch(patch_me_profile))
        .route("/me/avatar", post(post_me_avatar))
        .route("/me/cover", post(post_me_cover))
        .route("/me/sessions", get(list_me_sessions))
        .route("/me/sessions/{session_id}", axum::routing::delete(delete_me_session))
        .route("/auth/password-reset/request", post(password_reset_request))
        .route("/auth/password-reset/confirm", post(password_reset_confirm))
        // Multipart body cap: avatar/cover code re-validates at 5 MiB, but we cap the framework
        // intake at 6 MiB so a 5 GB upload is rejected before we touch RAM.
        .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
        .with_state(state);
    auth_routes.merge(profile_routes)
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

/// Build the session cookie (HttpOnly, Secure, SameSite=Lax) carrying the opaque session id, so
/// the browser persists the login and the gateway's auth middleware can resolve the caller.
fn session_cookie(s: &IssuedSession) -> String {
    let max_age = (s.expires_at - s.issued_at).num_seconds().max(0);
    format!(
        "dsoc_session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        s.id, max_age
    )
}

/// Query parameters for `GET /auth/me`.
#[derive(Debug, Deserialize)]
struct MeQuery {
    org_id: Uuid,
}

/// `POST /auth/register` — sovereign sign-up with e-mail + senha + CPF (ADR-0008).
async fn register(
    State(svc): State<Arc<ZitadelAuth>>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let org = OrgId::from_uuid(req.org_id);
    match svc.register(org, &req.email, &req.password, &req.cpf).await {
        Ok(session) => {
            let cookie = session_cookie(&session);
            (
                StatusCode::CREATED,
                [(header::SET_COOKIE, cookie)],
                Json(ApiResponse::ok(SessionDto::from(session))),
            )
                .into_response()
        }
        Err(error) => error_response(&error),
    }
}

/// `POST /auth/login` — authenticate with e-mail + senha.
async fn login(State(svc): State<Arc<ZitadelAuth>>, Json(req): Json<LoginRequest>) -> Response {
    let org = OrgId::from_uuid(req.org_id);
    match svc.login(org, &req.email, &req.password).await {
        Ok(session) => {
            let cookie = session_cookie(&session);
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie)],
                Json(ApiResponse::ok(SessionDto::from(session))),
            )
                .into_response()
        }
        Err(error) => error_response(&error),
    }
}

/// `POST /auth/logout` — invalidate the caller's session row AND tell the browser to drop the
/// HttpOnly cookie (which JS cannot clear on its own). Idempotent: missing/already-expired cookie
/// still returns 200, so a stale tab never sees an error trying to log out.
async fn logout(State(svc): State<Arc<ZitadelAuth>>, headers: HeaderMap) -> Response {
    let session_id = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_session_cookie)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    if let Some(sid) = session_id {
        if let Err(error) = svc.delete_session(sid).await {
            return error_response(&error);
        }
    }
    // Cookie attributes mirror the issuing site's so the browser overwrites the same cookie; the
    // `Max-Age=0` (and a past `Expires`) is what actually deletes it.
    let kill = "dsoc_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0";
    (
        StatusCode::OK,
        [(header::SET_COOKIE, kill)],
        Json(ApiResponse::<()>::ok(())),
    )
        .into_response()
}

/// Pull the `dsoc_session` value out of a `Cookie:` header — mirrors the gateway middleware's
/// parsing so logout sees the same cookie identity middleware does.
fn extract_session_cookie(cookie_header: &str) -> Option<&str> {
    cookie_header.split(';').find_map(|kv| {
        let kv = kv.trim();
        kv.strip_prefix("dsoc_session").and_then(|rest| rest.strip_prefix('='))
    })
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

/// End-user message (Portuguese — civic content policy) that never leaks internal detail. Conflict
/// reasons are mapped to a specific message when the service tagged them (e.g. `email_taken`,
/// `cpf_taken` during registration), so the user sees the exact field that collided.
fn message_for(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "Recurso não encontrado.",
        Error::Forbidden(_) => "Acesso negado.",
        Error::Unauthorized => "Não autenticado.",
        Error::Validation(_) => "Dados inválidos.",
        Error::Conflict(reason) => match reason.as_str() {
            "email_taken" => "Este e-mail já está cadastrado. Tente entrar na sua conta.",
            "cpf_taken" => "Este CPF já está cadastrado nesta plataforma.",
            "handle_taken" => "Esse nome de usuário já está em uso. Escolha outro.",
            _ => "Conflito de estado.",
        },
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

// ---------------------------------------------------------------------------
// Password reset — request + confirm flow (ADR-0010, W1).
// ---------------------------------------------------------------------------

/// Body for `POST /auth/password-reset/request`. The org is required because the same e-mail
/// can exist in multiple tenants (the existing register/login surfaces have the same shape).
#[derive(Debug, Deserialize)]
struct PasswordResetRequestBody {
    org_id: Uuid,
    email: String,
}

/// Body for `POST /auth/password-reset/confirm`.
#[derive(Debug, Deserialize)]
struct PasswordResetConfirmBody {
    token: String,
    password: String,
}

/// `POST /auth/password-reset/request` — start a reset. ALWAYS returns 200 OK regardless of
/// whether the e-mail is registered (enumeration-resistant). The user receives the e-mail iff
/// the account exists.
async fn password_reset_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasswordResetRequestBody>,
) -> Response {
    let svc = PasswordResetService::from_state(&state);
    // Best-effort source IP for the audit column. The deployment is behind Caddy, which sets
    // `X-Forwarded-For`; falling back to the RemoteAddr would require Axum's `ConnectInfo`,
    // which we do not wire here — `None` is acceptable.
    let request_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Err(error) = svc
        .request(OrgId::from_uuid(body.org_id), &body.email, request_ip)
        .await
    {
        // A hard storage failure is logged but still surfaces success at the wire — the user
        // experience must not depend on whether their e-mail exists.
        tracing::error!(error = ?error, "password-reset request storage failure");
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

/// `POST /auth/password-reset/confirm` — redeem a token and set a new password. Failures are
/// uniformly mapped to `Unauthorized` so the caller never learns whether the token was bad,
/// expired, or already used.
async fn password_reset_confirm(
    State(state): State<AppState>,
    Json(body): Json<PasswordResetConfirmBody>,
) -> Response {
    let svc = PasswordResetService::from_state(&state);
    match svc.confirm(&body.token, &body.password).await {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(error) => error_response(&error),
    }
}

// ---------------------------------------------------------------------------
// Profile (`/me`) — the social-substrate surface (ADR-0010).
// ---------------------------------------------------------------------------

/// `GET /me` — the authenticated citizen's profile. The identity comes from the gateway's
/// cookie-resolution middleware via `CallerId`; an unauthenticated request is rejected by the
/// extractor with 401 before we get here.
async fn get_me_profile(State(state): State<AppState>, caller: CallerId) -> Response {
    let svc = ProfileService::from_state(&state);
    match svc.get(caller.citizen, caller.org).await {
        Ok(profile) => (StatusCode::OK, Json(ApiResponse::ok(profile))).into_response(),
        Err(error) => error_response(&error),
    }
}

/// `POST /me/avatar` — multipart upload (`file` field). On success returns the refreshed
/// profile, so the client renders the new URL without a second round trip. The service
/// re-validates the bytes (the multipart Content-Type is untrusted), resizes to 512×512 PNG,
/// strips EXIF, and persists into MinIO.
async fn post_me_avatar(
    State(state): State<AppState>,
    caller: CallerId,
    multipart: axum::extract::Multipart,
) -> Response {
    upload_media(state, caller, multipart, MediaSlot::Avatar).await
}

/// `POST /me/cover` — mirror of `post_me_avatar` for the wide cover banner (1500×500).
async fn post_me_cover(
    State(state): State<AppState>,
    caller: CallerId,
    multipart: axum::extract::Multipart,
) -> Response {
    upload_media(state, caller, multipart, MediaSlot::Cover).await
}

/// Which profile media slot a request is targeting; small enum so the multipart handler is
/// shared between avatar and cover.
#[derive(Clone, Copy)]
enum MediaSlot {
    Avatar,
    Cover,
}

/// Shared multipart-handling body. Pulls the first part named `file` and hands its bytes off
/// to the service, which does all the validation.
async fn upload_media(
    state: AppState,
    caller: CallerId,
    mut multipart: axum::extract::Multipart,
    slot: MediaSlot,
) -> Response {
    let bytes = match read_file_part(&mut multipart).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return error_response(&Error::Validation(
                "envie o arquivo no campo `file`".to_owned(),
            ))
        }
        Err(error) => return error_response(&error),
    };
    let svc = ProfileService::from_state(&state);
    let result = match slot {
        MediaSlot::Avatar => svc.set_avatar(caller.citizen, caller.org, bytes).await,
        MediaSlot::Cover => svc.set_cover(caller.citizen, caller.org, bytes).await,
    };
    match result {
        Ok(profile) => (StatusCode::OK, Json(ApiResponse::ok(profile))).into_response(),
        Err(error) => error_response(&error),
    }
}

/// Read the FIRST multipart part whose name is `file`. Other parts are ignored. The body cap
/// applied at the router level guarantees a hostile upload never grows past 6 MiB.
async fn read_file_part(
    multipart: &mut axum::extract::Multipart,
) -> Result<Option<Vec<u8>>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| Error::Validation(format!("upload inválido: {e}")))?
    {
        if field.name() == Some("file") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| Error::Validation(format!("upload inválido: {e}")))?;
            return Ok(Some(bytes.to_vec()));
        }
    }
    Ok(None)
}

/// `GET /me/sessions` — list the caller's live sessions, with the one that issued THIS request
/// flagged as `current` so the UI can mark "(este dispositivo)" and disable revoke on it.
async fn list_me_sessions(
    State(state): State<AppState>,
    caller: CallerId,
    headers: HeaderMap,
) -> Response {
    let svc = ProfileService::from_state(&state);
    let current = current_session_id_from_headers(&headers);
    match svc.list_sessions(caller.citizen, current).await {
        Ok(sessions) => (StatusCode::OK, Json(ApiResponse::ok(sessions))).into_response(),
        Err(error) => error_response(&error),
    }
}

/// `DELETE /me/sessions/{session_id}` — revoke a session by id. Refuses to revoke the caller's
/// CURRENT session (use `POST /auth/logout` for that — it also clears the cookie). The DB
/// surface is gated by `citizen_id` so id-guessing across users yields a 404, never a successful
/// cross-user revoke.
async fn delete_me_session(
    State(state): State<AppState>,
    caller: CallerId,
    headers: HeaderMap,
    axum::extract::Path(session_id): axum::extract::Path<Uuid>,
) -> Response {
    let svc = ProfileService::from_state(&state);
    let current = current_session_id_from_headers(&headers);
    match svc.revoke_session(caller.citizen, session_id, current).await {
        Ok(()) => (StatusCode::NO_CONTENT, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(error) => error_response(&error),
    }
}

/// Pull the inbound `dsoc_session` cookie value (if any) and parse it as a UUID. Used by the
/// sessions list/revoke handlers to mark/protect the current device. Mirrors the gateway
/// middleware's cookie parsing exactly so the two surfaces always agree.
fn current_session_id_from_headers(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_session_cookie)
        .and_then(|raw| Uuid::parse_str(raw).ok())
}

/// `PATCH /me` — update display name / bio / handle / privacy. Returns the refreshed profile so
/// the client never has to re-fetch.
async fn patch_me_profile(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<ProfileUpdateDto>,
) -> Response {
    let svc = ProfileService::from_state(&state);
    let update = ProfileUpdate::from(body);
    match svc.update(caller.citizen, caller.org, update).await {
        Ok(profile) => (StatusCode::OK, Json(ApiResponse::ok(profile))).into_response(),
        Err(error) => error_response(&error),
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
