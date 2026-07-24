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
use crate::dto::{
    CreateSessionRequest, LoginRequest, MeDto, RegisterCandidateRequest, RegisterPoliticianRequest,
    RegisterRequest, SessionDto,
};
use crate::mandate_invite::{AcceptRequest, MandateInviteService};
use crate::password_reset::PasswordResetService;
use crate::profile::{ProfileService, ProfileUpdate};
use crate::service::{IssuedSession, ZitadelAuth};
use crate::signup_verify::{ConfirmOutcome, SignupVerifyService};

/// Build the routed service surface. Reads sovereign-issuer configuration from the environment
/// (never hardcoded — PLAN.md principle 8). When the issuer/JWKS is unconfigured the endpoints
/// reject with a dependency error rather than panicking.
pub fn routes(state: AppState) -> Router<()> {
    let svc = build_service(&state);
    // Profile routes carry AppState (the CallerId extractor pulls identity from middleware-set
    // headers, and the ProfileService is built per-request from state — cheap, no shared mutable
    // surface). Auth routes carry Arc<ZitadelAuth>; the two sub-routers are merged.
    let auth_routes = Router::new()
        .route("/auth/logout", post(logout))
        .route("/auth/session", post(create_session))
        .route("/auth/me", get(me))
        .with_state(svc);
    let profile_routes = Router::new()
        // Login movido pra profile_routes (AppState) pra ler request_ip +
        // gravar auth_login_attempt como parte do rate limit (P5.1).
        .route("/auth/login", post(login))
        .route("/me", get(get_me_profile).patch(patch_me_profile))
        .route("/me/avatar", post(post_me_avatar))
        .route("/me/cover", post(post_me_cover))
        .route("/me/sessions", get(list_me_sessions))
        .route(
            "/me/sessions/{session_id}",
            axum::routing::delete(delete_me_session),
        )
        .route("/profiles/{handle}", get(get_public_profile))
        // Cadastro com verificação de e-mail (0.25.0): register/register-politician
        // agora começam um pending signup e disparam um link de confirmação; a
        // conta só é materializada em /auth/register/confirm.
        .route("/auth/register", post(register))
        .route("/auth/register/politician", post(register_politician))
        .route("/auth/register/candidate", post(register_candidate))
        .route("/auth/register/confirm", post(register_confirm))
        .route("/auth/register/resend", post(register_resend))
        .route("/auth/password-reset/request", post(password_reset_request))
        .route("/auth/password-reset/confirm", post(password_reset_confirm))
        // Admin-driven mandate-invite bypass of F1.4 auto-proof (crates/platform/auth/src/mandate_invite.rs).
        // The path `/mandates/{id}/invites` is DELIBERATELY distinct from the pre-existing
        // `/mandates/{id}/invitations` owned by dsoc-mandates (a different, mandate-lifecycle flow).
        .route("/mandates/{mandate_id}/invites", post(send_mandate_invite))
        .route("/mandate-invites/{token}", get(get_mandate_invite))
        .route(
            "/mandate-invites/{token}/accept",
            post(accept_mandate_invite),
        )
        .route(
            "/mandate-invites/{token}/revoke",
            post(revoke_mandate_invite),
        )
        // Multipart body cap: avatar/cover code re-validates at 5 MiB, but we cap the framework
        // intake at 6 MiB so a 5 GB upload is rejected before we touch RAM.
        .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
        .with_state(state);
    auth_routes.merge(profile_routes)
}

/// Constrói o `MandateInviteService` completo (com o auth backend) a partir
/// do `AppState`. Público porque o gateway usa no envio em lote da campanha
/// de convites (0.34.0) — o service é o MESMO do fluxo individual, com todas
/// as guardas (Forbidden pra não-admin, token hasheado, TTL).
pub fn mandate_invite_service(state: &AppState) -> MandateInviteService {
    MandateInviteService::from_state(state, build_service(state))
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
            // ADR-0012: Zitadel abandonado — cred sovereign é o único path ativo. O rejeitador
            // é o comportamento esperado se alguém tentar `POST /auth/session` (path morto).
            tracing::info!(
                "AUTH_OIDC_* unset — OIDC path dormant (ADR-0012). Cred sovereign (email+senha+CPF) é o único auth ativo."
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

/// Public convenience — build a `ZitadelAuth` service from an `AppState`.
/// The gateway's Mastodon Client API layer uses this for the OAuth password
/// grant (`POST /oauth/token`); the login path only needs db + clock, so the
/// (possibly-unconfigured) OIDC validator is fine.
#[must_use]
pub fn zitadel_from_state(state: &AppState) -> ZitadelAuth {
    build_zitadel(state.db.clone(), state.clock.clone(), state.bus.clone())
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

/// Extract "IP de origem" do X-Forwarded-For (o gateway roda atrás do Caddy).
/// Mesma leitura que o `password_reset_request` faz — deduplicado aqui.
fn caller_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Corpo de resposta pros dois `/auth/register` — 202 Accepted, sem sessão ainda.
/// O front usa `status` pra saber que precisa mostrar a tela "verifique seu e-mail".
#[derive(Debug, serde::Serialize)]
struct SignupPendingDto {
    status: &'static str,
    email: String,
}

/// `POST /auth/register` — inicia o cadastro. Grava um pending_signup e
/// dispara e-mail com link `/confirmar-conta?token=…`. **Não cria a conta**
/// nem retorna sessão — o front navega pra tela de "verifique seu e-mail".
async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let svc = SignupVerifyService::from_state(&state);
    let org = OrgId::from_uuid(req.org_id);
    let ip = caller_ip(&headers);
    match svc
        .request_cidadao(org, &req.email, &req.password, &req.cpf, ip.as_deref())
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(SignupPendingDto {
                status: "verification_sent",
                email: req.email.trim().to_lowercase(),
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// `POST /auth/register/politician` — inicia o cadastro F1.3/F1.4. Antes de
/// gravar o pending, valida `body.email == mandate.public_email`. Mesmo shape
/// de resposta do `/auth/register`.
async fn register_politician(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterPoliticianRequest>,
) -> Response {
    let svc = SignupVerifyService::from_state(&state);
    let org = OrgId::from_uuid(req.org_id);
    let ip = caller_ip(&headers);
    match svc
        .request_politico(
            org,
            &req.email,
            &req.password,
            &req.cpf,
            req.mandate_id,
            ip.as_deref(),
        )
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(SignupPendingDto {
                status: "verification_sent",
                email: req.email.trim().to_lowercase(),
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// `POST /auth/register/candidate` — cadastro de candidato(a) SEM mandato
/// (0526). Valida os metadados da candidatura, grava a pending e envia o
/// link de verificação. Mesmo shape de resposta do `/auth/register`.
async fn register_candidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterCandidateRequest>,
) -> Response {
    let svc = SignupVerifyService::from_state(&state);
    let org = OrgId::from_uuid(req.org_id);
    let ip = caller_ip(&headers);
    let meta = crate::signup_verify::CandidateMeta {
        display_name: req.display_name,
        office: req.office,
        uf: req.uf,
        municipio: req.municipio,
        party_sigla: req.party_sigla,
        number: req.number,
    };
    match svc
        .request_candidato(
            org,
            &req.email,
            &req.password,
            &req.cpf,
            meta,
            ip.as_deref(),
        )
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(SignupPendingDto {
                status: "verification_sent",
                email: req.email.trim().to_lowercase(),
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// Corpo de `POST /auth/register/confirm`.
#[derive(Debug, Deserialize)]
struct RegisterConfirmBody {
    token: String,
}

/// Corpo de `POST /auth/register/resend`. Mesmo shape do password-reset
/// request — org_id + email, sem senha (a gente reusa a da pending).
#[derive(Debug, Deserialize)]
struct RegisterResendBody {
    org_id: Uuid,
    email: String,
}

/// `POST /auth/register/resend` — reenvia o link de verificação. **Sempre**
/// responde 200 OK: se não houver pending viva pra esse e-mail, o front
/// vê sucesso mesmo assim (enumeration-safe — o mesmo padrão do reset).
async fn register_resend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterResendBody>,
) -> Response {
    let svc = SignupVerifyService::from_state(&state);
    let ip = caller_ip(&headers);
    if let Err(error) = svc
        .resend(OrgId::from_uuid(body.org_id), &body.email, ip.as_deref())
        .await
    {
        tracing::error!(error = ?error, "signup resend storage failure");
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

/// `POST /auth/register/confirm` — redime o token de verificação e materializa
/// citizen + credential + sessão numa tx. Retorna o mesmo `SessionDto` do
/// login pra que o front redirecione direto pra o painel/bem-vinda como se
/// fosse um cadastro que "só demorou um passo".
async fn register_confirm(
    State(state): State<AppState>,
    Json(body): Json<RegisterConfirmBody>,
) -> Response {
    let svc = SignupVerifyService::from_state(&state);
    match svc.confirm(&body.token).await {
        Ok(ConfirmOutcome::Session(session)) => {
            let cookie = session_cookie(&session);
            (
                StatusCode::CREATED,
                [(header::SET_COOKIE, cookie)],
                Json(ApiResponse::ok(SessionDto::from(*session))),
            )
                .into_response()
        }
        // Instância com revisão manual ligada: conta criada, sem sessão.
        // O front mostra "aguardando aprovação" no lugar do redirect.
        Ok(ConfirmOutcome::PendingReview { email }) => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::ok(SignupPendingDto {
                status: "pending_review",
                email,
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// Máximo de tentativas de login por IP na janela de 1h (P5.1). Override
/// via `AUTH_LOGIN_RATE_MAX_PER_HOUR`. Uma pessoa comum faz 3-5 logins/dia;
/// 10/h já é sinal de bot/força-bruta.
const DEFAULT_LOGIN_RATE_MAX_PER_HOUR: i64 = 10;

/// `POST /auth/login` — authenticate with e-mail + senha. Rate-limitado por
/// IP: 10 tentativas/hora (sucesso + falha), configurável via env. Toda
/// tentativa é auditada em `auth_login_attempt`.
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    let ip = caller_ip(&headers);
    // Rate check. Quem não vem com XFF passa; a política de "no XFF = trust"
    // é uma decisão consciente (o front está sempre atrás do Caddy que
    // preenche o header — se veio None, provavelmente é um teste local).
    if let Some(ip_str) = ip.as_deref() {
        let since = state.clock.now() - chrono::Duration::hours(1);
        match crate::queries::login_attempt_count_by_ip_since(&state.db, ip_str, since).await {
            Ok(count) => {
                let max = std::env::var("AUTH_LOGIN_RATE_MAX_PER_HOUR")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|&v: &i64| v > 0)
                    .unwrap_or(DEFAULT_LOGIN_RATE_MAX_PER_HOUR);
                if count >= max {
                    return error_response(&Error::RateLimit(
                        "muitas tentativas de login deste IP na última hora".to_owned(),
                    ));
                }
            }
            Err(err) => {
                tracing::error!(error = ?err, "login rate check DB failure");
                // Não é catastrófico — segue pro login. Falha aberta em cima
                // de storage down é aceitável (o login vai falhar de qualquer
                // jeito se DB não responder).
            }
        }
    }

    let svc = zitadel_from_state(&state);
    let org = OrgId::from_uuid(req.org_id);
    let result = svc.login(org, &req.email, &req.password).await;
    // Grava a tentativa antes de responder (mesmo em erro) — audit + rate.
    if let Some(ip_str) = ip.as_deref() {
        let outcome = if result.is_ok() { "ok" } else { "fail" };
        if let Err(err) = crate::queries::login_attempt_record(&state.db, ip_str, outcome).await {
            tracing::warn!(error = ?err, "login attempt audit failed");
        }
    }
    match result {
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
        kv.strip_prefix("dsoc_session")
            .and_then(|rest| rest.strip_prefix('='))
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
        Error::RateLimit(_) => StatusCode::TOO_MANY_REQUESTS,
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
        Error::RateLimit(_) => "Muitas tentativas. Aguarde um pouco antes de tentar novamente.",
        _ => "Erro interno do servidor.",
    }
}

fn error_response(error: &Error) -> Response {
    // Log internal detail server-side only; the body carries a stable code + safe message.
    // Debug (`?error`) walks the `#[source]` chain — Display esconde a causa raiz (por design,
    // pra não vazar na resposta), o que atrapalha o diagnóstico nos logs.
    if matches!(error, Error::Storage(_) | Error::Dependency { .. }) {
        tracing::error!(code = error.code(), detail = ?error, "auth request failed");
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
    let request_ip = caller_ip(&headers);
    if let Err(error) = svc
        .request(
            OrgId::from_uuid(body.org_id),
            &body.email,
            request_ip.as_deref(),
        )
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

/// Query for `GET /profiles/{handle}` — needs to know which org (tenant) the handle lives in.
#[derive(Debug, Deserialize)]
struct PublicProfileQuery {
    org_id: Uuid,
}

/// `GET /profiles/{handle}?org_id=` — public read of a citizen's profile by their chosen
/// handle. Gated by `is_public=true` inside the service; hidden citizens 404 just like
/// unknown ones (no signal that the handle exists). Drives the human `/perfil/{handle}`
/// page and the follow-back render on federation cards.
async fn get_public_profile(
    State(state): State<AppState>,
    axum::extract::Path(handle): axum::extract::Path<String>,
    Query(q): Query<PublicProfileQuery>,
) -> Response {
    let svc = ProfileService::from_state(&state);
    match svc
        .find_public_by_handle(OrgId::from_uuid(q.org_id), &handle)
        .await
    {
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
async fn read_file_part(multipart: &mut axum::extract::Multipart) -> Result<Option<Vec<u8>>> {
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
    match svc
        .revoke_session(caller.citizen, session_id, current)
        .await
    {
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

// ---------------------------------------------------------------------------
// Admin-driven mandate invite (see `crate::mandate_invite`).
// ---------------------------------------------------------------------------

/// Body for `POST /mandates/{mandate_id}/invites`.
#[derive(Debug, Deserialize)]
struct SendMandateInviteBody {
    email: String,
}

/// Public shape returned to the inviter — deliberately excludes the plaintext token (that
/// value only exists in the outgoing e-mail).
#[derive(Debug, serde::Serialize)]
struct SentInviteDto {
    id: Uuid,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Public shape for the accept-page summary — no e-mail, no inviter identity.
#[derive(Debug, serde::Serialize)]
struct MandateInviteSummaryDto {
    mandate_display_name: String,
    party: Option<String>,
    uf: Option<String>,
    office: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Body for `POST /mandate-invites/{token}/accept`.
#[derive(Debug, Deserialize)]
struct AcceptMandateInviteBody {
    password: String,
    cpf: String,
    display_name: String,
}

/// `POST /mandates/{mandate_id}/invites` — dispatch an invite. Any authenticated citizen may
/// call today; TODO gate by "admin de partido/plataforma" role when that model lands.
async fn send_mandate_invite(
    State(state): State<AppState>,
    caller: CallerId,
    axum::extract::Path(mandate_id): axum::extract::Path<Uuid>,
    Json(body): Json<SendMandateInviteBody>,
) -> Response {
    // Authorization: `MandateInviteService::send` enforces `invite_authorized` (platform
    // owner/admin OR admin of the mandate's party; `moderador` does NOT qualify) and returns
    // Forbidden otherwise. The CallerId extractor already proved a live session.
    let svc = MandateInviteService::from_state(&state, build_service(&state));
    match svc
        .send(caller.org, caller.citizen, mandate_id, &body.email)
        .await
    {
        Ok(sent) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(SentInviteDto {
                id: sent.id,
                expires_at: sent.expires_at,
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// `GET /mandate-invites/{token}` — public summary used by the accept page to render context.
/// Any bad/expired/revoked/accepted token collapses to 404 (no signal on which case).
async fn get_mandate_invite(
    State(state): State<AppState>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> Response {
    let svc = MandateInviteService::from_state(&state, build_service(&state));
    match svc.summary(&token).await {
        Ok(s) => (
            StatusCode::OK,
            Json(ApiResponse::ok(MandateInviteSummaryDto {
                mandate_display_name: s.mandate_display_name,
                party: s.party,
                uf: s.uf,
                office: s.office,
                expires_at: s.expires_at,
            })),
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// `POST /mandate-invites/{token}/accept` — public accept endpoint. Creates the citizen +
/// credential + directory identity binding + session, all atomically.
async fn accept_mandate_invite(
    State(state): State<AppState>,
    axum::extract::Path(token): axum::extract::Path<String>,
    Json(body): Json<AcceptMandateInviteBody>,
) -> Response {
    let svc = MandateInviteService::from_state(&state, build_service(&state));
    let req = AcceptRequest {
        password: body.password,
        cpf: body.cpf,
        display_name: body.display_name,
    };
    match svc.accept(&token, req).await {
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

/// `POST /mandate-invites/{token}/revoke` — the inviter (only) invalidates a pending invite.
async fn revoke_mandate_invite(
    State(state): State<AppState>,
    caller: CallerId,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> Response {
    let svc = MandateInviteService::from_state(&state, build_service(&state));
    match svc.revoke(caller.org, caller.citizen, &token).await {
        Ok(()) => (StatusCode::NO_CONTENT, Json(ApiResponse::<()>::ok(()))).into_response(),
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
