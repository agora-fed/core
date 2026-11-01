//! Sign-in through the instance's OIDC provider (authorization code + PKCE).
//!
//! Accounts are created on first sign-in. A provider may host several organizations, so
//! `SSO_ALLOWED_ORG_ID` limits who may enter. Set `SSO_ISSUER`, `SSO_CLIENT_ID`,
//! `SSO_CLIENT_SECRET` and `SSO_ALLOWED_ORG_ID` to enable; `SSO_LABEL` names the button.

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::admin_branding::DEFAULT_ORG_UUID;

const STATE_COOKIE: &str = "dsoc_sso";
const ORG_CLAIM: &str = "urn:zitadel:iam:user:resourceowner:id";
const DEFAULT_SESSION_TTL_SECS: i64 = 3600;

pub(crate) fn root_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/auth/sso/start", get(start))
        .route("/auth/sso/callback", get(callback))
        .with_state(state)
}

pub(crate) fn api_routes() -> Router<()> {
    Router::new().route("/auth/sso/status", get(status))
}

#[derive(Debug, Clone)]
struct Cfg {
    issuer: String,
    client_id: String,
    client_secret: String,
    allowed_org: String,
    redirect_uri: String,
    label: String,
}

impl Cfg {
    fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let origin = var("PUBLIC_ORIGIN")?;
        Some(Self {
            issuer: var("SSO_ISSUER")?.trim_end_matches('/').to_owned(),
            client_id: var("SSO_CLIENT_ID")?,
            client_secret: var("SSO_CLIENT_SECRET")?,
            allowed_org: var("SSO_ALLOWED_ORG_ID")?,
            redirect_uri: format!("{}/auth/sso/callback", origin.trim_end_matches('/')),
            label: var("SSO_LABEL").unwrap_or_else(|| "Entrar com sua conta".to_owned()),
        })
    }
}

/// `SSO_ONLY=true` makes the provider the only way to get an account or sign in.
pub(crate) fn sso_only() -> bool {
    Cfg::from_env().is_some() && std::env::var("SSO_ONLY").is_ok_and(|v| v == "true")
}

async fn status() -> Response {
    let cfg = Cfg::from_env();
    Json(ApiResponse::ok(serde_json::json!({
        "enabled": cfg.is_some(),
        "only": sso_only(),
        "label": cfg.map(|c| c.label),
    })))
    .into_response()
}

fn is_signup_page(path: &str) -> bool {
    matches!(path, "/cadastrar" | "/cadastrar/")
}

/// Send the sign-up page to the provider, where accounts are created.
pub(crate) async fn signup_page_redirect(req: Request, next: Next) -> Response {
    if sso_only() && is_signup_page(req.uri().path()) {
        let target = std::env::var("SSO_SIGNUP_URL").unwrap_or_else(|_| "/entrar/".to_owned());
        return Redirect::temporary(&target).into_response();
    }
    next.run(req).await
}

/// The break-glass password login, kept so that an outage at the provider can't lock
/// administrators out.
pub(crate) async fn may_use_password(db: &PgPool, email: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM auth_credential a
              JOIN admin_role_binding r ON r.citizen_id = a.citizen_id AND r.org_id = a.org_id
             WHERE a.email = $1 AND r.role IN ('owner', 'admin'))",
    )
    .bind(email.trim().to_lowercase())
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Only paths on this site, so the login can't be used to send people elsewhere.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_owned(),
        _ => "/".to_owned(),
    }
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct StartQuery {
    next: Option<String>,
}

async fn start(Query(q): Query<StartQuery>) -> Response {
    let Some(cfg) = Cfg::from_env() else {
        return login_error("indisponivel");
    };
    let (state, nonce, verifier) = (random_token(), random_token(), random_token());
    let next = URL_SAFE_NO_PAD.encode(safe_next(q.next.as_deref()));
    let cookie = format!(
        "{STATE_COOKIE}={state}.{nonce}.{verifier}.{next}; HttpOnly; Secure; SameSite=Lax; Path=/auth/sso; Max-Age=600"
    );
    let scope = format!(
        "openid profile email urn:zitadel:iam:org:id:{} urn:zitadel:iam:user:resourceowner",
        cfg.allowed_org
    );
    let url = format!(
        "{}/oauth/v2/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
        cfg.issuer,
        encode(&cfg.client_id),
        encode(&scope),
        encode(&cfg.redirect_uri),
        state,
        nonce,
        pkce_challenge(&verifier),
    );
    (
        StatusCode::FOUND,
        [(header::SET_COOKIE, cookie), (header::LOCATION, url)],
    )
        .into_response()
}

struct PendingLogin {
    state: String,
    nonce: String,
    verifier: String,
    next: String,
}

fn read_cookie(headers: &HeaderMap) -> Option<PendingLogin> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let value = crate::cookie_value(raw, STATE_COOKIE)?;
    let mut parts = value.split('.');
    let (state, nonce, verifier, next) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let next = String::from_utf8(URL_SAFE_NO_PAD.decode(next).ok()?).ok()?;
    Some(PendingLogin {
        state: state.to_owned(),
        nonce: nonce.to_owned(),
        verifier: verifier.to_owned(),
        next: safe_next(Some(&next)),
    })
}

fn login_error(code: &str) -> Response {
    Redirect::to(&format!("/entrar/?sso_erro={code}")).into_response()
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    nonce: Option<String>,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    name: Option<String>,
    preferred_username: Option<String>,
    #[serde(rename = "urn:zitadel:iam:user:resourceowner:id")]
    org: Option<String>,
}

/// Read the claims of a token whose signature was already verified.
fn parse_claims(id_token: &str) -> Option<Claims> {
    let payload = id_token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

async fn exchange_code(cfg: &Cfg, code: &str, verifier: &str) -> Result<String, reqwest::Error> {
    let body = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("code_verifier", verifier),
    ];
    let res = reqwest::Client::new()
        .post(format!("{}/oauth/v2/token", cfg.issuer))
        .basic_auth(&cfg.client_id, Some(&cfg.client_secret))
        .form(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<TokenResponse>()
        .await?;
    Ok(res.id_token)
}

async fn verified_claims(state: &AppState, cfg: &Cfg, id_token: &str) -> Option<Claims> {
    let validator = dsoc_auth::oidc_validator(
        format!("{}/oauth/v2/keys", cfg.issuer),
        cfg.issuer.clone(),
        cfg.client_id.clone(),
    );
    if let Err(err) = validator.validate(id_token, state.clock.now()).await {
        tracing::warn!(?err, "sso: id_token rejected");
        return None;
    }
    parse_claims(id_token)
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(cfg) = Cfg::from_env() else {
        return login_error("indisponivel");
    };
    if let Some(err) = q.error.as_deref() {
        tracing::warn!(err, "sso: provider returned an error");
        return login_error("provedor");
    }
    let Some(pending) = read_cookie(&headers) else {
        return login_error("sessao");
    };
    let (Some(code), Some(returned)) = (q.code, q.state) else {
        return login_error("resposta");
    };
    if returned != pending.state {
        return login_error("sessao");
    }
    let id_token = match exchange_code(&cfg, &code, &pending.verifier).await {
        Ok(token) => token,
        Err(err) => {
            tracing::error!(?err, "sso: token exchange failed");
            return login_error("provedor");
        }
    };
    let Some(claims) = verified_claims(&state, &cfg, &id_token).await else {
        return login_error("token");
    };
    if claims.nonce.as_deref() != Some(pending.nonce.as_str()) {
        return login_error("token");
    }
    if claims.org.as_deref() != Some(cfg.allowed_org.as_str()) {
        tracing::warn!(claim = ORG_CLAIM, "sso: account from another organization");
        return login_error("organizacao");
    }
    let session = match sign_in(&state.db, &claims).await {
        Ok(Some(session)) => session,
        Ok(None) => return login_error("conta"),
        Err(err) => {
            tracing::error!(?err, "sso: sign-in failed");
            return login_error("falha");
        }
    };
    let session_cookie = format!(
        "dsoc_session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        session.0, session.1
    );
    let clear =
        format!("{STATE_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/auth/sso; Max-Age=0");
    let location = format!("/entrar/?sso=ok&next={}", encode(&pending.next));
    (
        StatusCode::FOUND,
        AppendHeaders([
            (header::SET_COOKIE, session_cookie),
            (header::SET_COOKIE, clear),
            (header::LOCATION, location),
        ]),
    )
        .into_response()
}

/// Valid handles match the `citizen_handle_format` database constraint.
fn usable_handle(preferred: Option<&str>) -> Option<String> {
    let handle = preferred?.split('@').next()?.trim();
    let valid_length = (3..=32).contains(&handle.len());
    let valid_chars = handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    (valid_length && valid_chars && !handle.contains("..")).then(|| handle.to_owned())
}

fn display_name(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    (!name.is_empty()).then(|| name.chars().take(80).collect())
}

/// Whether a provider's subject may sign in at all. `system:` names internal citizens with no
/// credentials, which a misconfigured provider must not be able to enter.
pub(crate) fn subject_may_sign_in(subject: &str) -> bool {
    !subject.starts_with("system:")
}

/// Find or create the account and open a session. `None` means the account may not enter.
async fn sign_in(db: &PgPool, claims: &Claims) -> Result<Option<(Uuid, i64)>, sqlx::Error> {
    if !subject_may_sign_in(&claims.sub) {
        return Ok(None);
    }
    let org = DEFAULT_ORG_UUID;
    let email = claims
        .email
        .as_deref()
        .filter(|_| claims.email_verified)
        .map(|e| e.trim().to_lowercase());
    let mut tx = db.begin().await?;

    let mut citizen: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM citizen WHERE org_id = $1 AND oidc_subject = $2")
            .bind(org)
            .bind(&claims.sub)
            .fetch_optional(&mut *tx)
            .await?;

    // Only a verified address may claim an existing account.
    if let (None, Some(email)) = (citizen, email.as_deref()) {
        citizen = sqlx::query_scalar(
            "UPDATE citizen c SET oidc_subject = $3
               FROM auth_credential a
              WHERE a.citizen_id = c.id AND a.org_id = $1 AND a.email = $2
                AND c.oidc_subject IS NULL
          RETURNING c.id",
        )
        .bind(org)
        .bind(email)
        .bind(&claims.sub)
        .fetch_optional(&mut *tx)
        .await?;
    }

    let citizen = match citizen {
        Some(id) => id,
        None => create_account(&mut tx, org, claims, email.as_deref()).await?,
    };

    if email.is_some() {
        sqlx::query(
            "UPDATE citizen SET verification_level = 'email'
              WHERE id = $1 AND verification_level = 'anonymous'",
        )
        .bind(citizen)
        .execute(&mut *tx)
        .await?;
    }

    let blocked: bool = sqlx::query_scalar(
        "SELECT suspended_at IS NOT NULL OR deleted_at IS NOT NULL OR pending_review
           FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .fetch_one(&mut *tx)
    .await?;
    if blocked {
        return Ok(None);
    }

    let ttl = std::env::var("AUTH_SESSION_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &i64| *v > 0)
        .unwrap_or(DEFAULT_SESSION_TTL_SECS);
    let session = Uuid::now_v7();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO auth_session
             (id, org_id, citizen_id, oidc_subject, issued_at, expires_at, public_handle, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $5)",
    )
    .bind(session)
    .bind(org)
    .bind(citizen)
    .bind(&claims.sub)
    .bind(now)
    .bind(now + chrono::Duration::seconds(ttl))
    .bind(format!("u-{}", citizen.simple()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some((session, ttl)))
}

async fn create_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    claims: &Claims,
    email: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let name = display_name(claims.name.as_deref());
    let level = if email.is_some() {
        "email"
    } else {
        "anonymous"
    };
    let wanted = usable_handle(claims.preferred_username.as_deref());
    let handle = match wanted {
        Some(handle) => {
            let taken: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM citizen WHERE org_id = $1 AND lower(handle) = lower($2))",
            )
            .bind(org)
            .bind(&handle)
            .fetch_one(&mut **tx)
            .await?;
            (!taken).then_some(handle)
        }
        None => None,
    }
    .unwrap_or_else(|| format!("cidadao-{}", &id.as_simple().to_string()[..8]));

    sqlx::query(
        "INSERT INTO citizen
             (id, org_id, oidc_subject, verification_level, created_at, handle, display_name, is_public)
         VALUES ($1, $2, $3, $4, now(), $5, $6, false)",
    )
    .bind(id)
    .bind(org)
    .bind(&claims.sub)
    .bind(level)
    .bind(&handle)
    .bind(&name)
    .execute(&mut **tx)
    .await?;

    // No usable password: this keeps the address for notifications and account linking.
    if let Some(email) = email {
        sqlx::query(
            "INSERT INTO auth_credential
                 (id, citizen_id, org_id, email, password_hash, cpf_status, created_at)
             VALUES ($1, $2, $3, $4, '!', 'unverified', now())
             ON CONFLICT (org_id, email) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(org)
        .bind(email)
        .execute(&mut **tx)
        .await?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_signup_page_redirects() {
        assert!(is_signup_page("/cadastrar"));
        assert!(is_signup_page("/cadastrar/"));
        assert!(!is_signup_page("/cadastrar-algo"));
    }

    #[test]
    fn a_system_subject_can_never_sign_in() {
        assert!(!subject_may_sign_in("system:fediverse"));
        assert!(!subject_may_sign_in("system:socrates"));
        assert!(subject_may_sign_in("397439380358169871"));
    }

    #[test]
    fn next_stays_on_this_site() {
        assert_eq!(safe_next(Some("/admin/?x=1")), "/admin/?x=1");
        for hostile in ["https://evil.example", "//evil.example", "/\\evil.example"] {
            assert_eq!(safe_next(Some(hostile)), "/", "{hostile}");
        }
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn pkce_matches_the_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn handles_follow_the_database_rule() {
        assert_eq!(
            usable_handle(Some("navigator")).as_deref(),
            Some("navigator")
        );
        assert_eq!(
            usable_handle(Some("ana@organica.social")).as_deref(),
            Some("ana")
        );
        for bad in ["m", "a..b", "com espaco", "ação"] {
            assert_eq!(usable_handle(Some(bad)), None, "{bad}");
        }
        assert_eq!(usable_handle(None), None);
    }

    #[test]
    fn claims_read_the_organization() {
        let payload = URL_SAFE_NO_PAD.encode(
            r#"{"sub":"1","email":"a@b.c","email_verified":true,"urn:zitadel:iam:user:resourceowner:id":"42"}"#,
        );
        let claims = parse_claims(&format!("h.{payload}.s")).unwrap();
        assert_eq!(claims.org.as_deref(), Some("42"));
        assert!(claims.email_verified);
    }

    #[test]
    fn missing_verification_is_treated_as_unverified() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"1","email":"a@b.c"}"#);
        assert!(
            !parse_claims(&format!("h.{payload}.s"))
                .unwrap()
                .email_verified
        );
    }
}
