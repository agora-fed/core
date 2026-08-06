//! Login with gov.br (OIDC Authorization Code Flow) — 0.25.0-fediverse.
//!
//! Fluxo:
//! 1. The front end calls `GET /auth/govbr/start` → redirects to gov.br with
//!    state+nonce assinados no cookie.
//! 2. gov.br authenticates the citizen (password/2FA/biometrics) and returns to
//!    `GET /auth/govbr/callback?code=...&state=...`.
//! 3. Callback:
//!    a. valida state (CSRF) contra o cookie.
//!    b. exchanges the code for tokens at gov.br's `/token`.
//!    c. valida id_token JWT (issuer, aud, exp).
//!    d. reads `sub`, `name`, `email`, `amr` (assurance).
//!    e. upserts into `citizen` (govbr_sub as the key):
//!       - a new account: created with display_name=name, legal_name=name,
//!         is_public=true, verification_level=directory.
//!       - existente: atualiza legal_name e confiabilidade.
//!
//!    f. creates the session + cookie and redirects to /feed.
//!
//! Config (env, prod k8s Secret):
//! - `GOVBR_CLIENT_ID`, `GOVBR_CLIENT_SECRET` (required — without them the path returns 503).
//! - `GOVBR_ISSUER` (default `https://sso.acesso.gov.br`; staging
//!   `https://sso.staging.acesso.gov.br`).
//! - `GOVBR_REDIRECT_URI` (default `<PUBLIC_ORIGIN>/auth/govbr/callback`).
//!
//! Security:
//! - `state` (32 bytes random) + `nonce` (32 bytes random) em cookie HttpOnly
//!   with a 10-minute TTL. The callback validates BOTH.
//! - `id_token` checked against gov.br's JWKS (cached for 1 h).
//! - `client_secret` NUNCA sai do backend.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use rand::RngCore;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Whether the `id_token` signature is actually verified against gov.br's JWKS.
///
/// `false`, and the callback refuses while it is. Flipping this to `true` without
/// implementing the JWKS check (issue #7) re-opens an authentication path that
/// accepts any syntactically valid JWT with an attacker-chosen `sub`.
const ID_TOKEN_IS_VERIFIED: bool = false;

/// Routes mounted at the gateway ROOT (outside `/api/v1`) — gov.br requires
/// that `redirect_uri` be exactly `<origin>/auth/govbr/callback`.
pub fn root_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/auth/govbr/start", get(start))
        .route("/auth/govbr/callback", get(callback))
        .with_state(state)
}

/// The "status" route (`GET /api/v1/auth/govbr/status`) — used by the front end to
/// decide whether to show the "Sign in with gov.br" button.
pub fn api_routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/auth/govbr/status", get(status))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Cfg {
    client_id: String,
    client_secret: String,
    issuer: String,
    redirect_uri: String,
}

impl Cfg {
    fn from_env() -> Option<Self> {
        let client_id = std::env::var("GOVBR_CLIENT_ID")
            .ok()
            .filter(|s| !s.is_empty())?;
        let client_secret = std::env::var("GOVBR_CLIENT_SECRET")
            .ok()
            .filter(|s| !s.is_empty())?;
        let issuer = std::env::var("GOVBR_ISSUER")
            .unwrap_or_else(|_| "https://sso.acesso.gov.br".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let public_origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let redirect_uri = std::env::var("GOVBR_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{}/auth/govbr/callback", public_origin));
        Some(Cfg {
            client_id,
            client_secret,
            issuer,
            redirect_uri,
        })
    }
}

// ---------------------------------------------------------------------------
// GET /auth/govbr/status — the front end uses it to decide whether to show the button
// ---------------------------------------------------------------------------

async fn status() -> Response {
    let enabled = Cfg::from_env().is_some();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "enabled": enabled }))),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /auth/govbr/start
// ---------------------------------------------------------------------------

async fn start(State(_state): State<AppState>) -> Response {
    let Some(cfg) = Cfg::from_env() else {
        return unavailable();
    };
    let state_tok = urlsafe_random(32);
    let nonce_tok = urlsafe_random(32);
    // The cookie combines both as `state|nonce`, HttpOnly, 10 min.
    let cookie = format!(
        "dsoc_govbr={}|{}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=600",
        state_tok, nonce_tok
    );
    let scope = "openid profile email govbr_confiabilidades";
    let authorize_url = format!(
        "{}/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&nonce={}",
        cfg.issuer,
        urlencode(&cfg.client_id),
        urlencode(scope),
        urlencode(&cfg.redirect_uri),
        state_tok,
        nonce_tok,
    );
    (
        StatusCode::FOUND,
        [
            (header::SET_COOKIE, cookie),
            (header::LOCATION, authorize_url),
        ],
        "",
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /auth/govbr/callback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(cfg) = Cfg::from_env() else {
        return unavailable();
    };
    if let Some(err) = q.error.as_deref() {
        tracing::warn!(?err, ?q.error_description, "gov.br devolveu erro");
        return Redirect::to("/entrar?govbr_erro=1").into_response();
    }
    let Some(code) = q.code else {
        return Redirect::to("/entrar?govbr_erro=missing_code").into_response();
    };
    let Some(state_returned) = q.state else {
        return Redirect::to("/entrar?govbr_erro=missing_state").into_response();
    };
    let (cookie_state, cookie_nonce) = match read_govbr_cookie(&headers) {
        Some(v) => v,
        None => return Redirect::to("/entrar?govbr_erro=cookie").into_response(),
    };
    if cookie_state != state_returned {
        return Redirect::to("/entrar?govbr_erro=state_mismatch").into_response();
    }

    // 1. Exchange the code for tokens.
    let token_resp = match exchange_code(&cfg, &code).await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "gov.br token exchange falhou");
            return Redirect::to("/entrar?govbr_erro=token").into_response();
        }
    };

    // DISABLED (issue #7). The `id_token` below is DECODED, never cryptographically
    // VERIFIED against gov.br's JWKS — the signature is not checked at all. The
    // original note argued that TLS plus "gov.br is the only signer" made that
    // acceptable; it does not. Anything that reaches this handler with a
    // syntactically valid JWT gets a session, and `sub` is attacker-chosen.
    //
    // The flow was already inert in practice (no GOVBR_CLIENT_ID in production ⇒
    // 503 before this point), which made the hole LATENT: setting two environment
    // variables would have silently enabled an authentication path that trusts an
    // unverified token. The product decision (2026-08-06) is that gov.br is not
    // being pursued, so rather than leave that armed, the callback refuses here.
    //
    // To revive gov.br: implement JWKS verification (issue #7) and delete this
    // block — NOT the other way round.
    if !ID_TOKEN_IS_VERIFIED {
        tracing::error!("gov.br callback refused: id_token verification is unimplemented (#7)");
        return Redirect::to("/entrar?govbr_erro=indisponivel").into_response();
    }

    let claims = match decode_id_token(&token_resp.id_token) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(?err, "id_token decode falhou");
            return Redirect::to("/entrar?govbr_erro=id_token").into_response();
        }
    };
    if claims.nonce.as_deref() != Some(cookie_nonce.as_str()) {
        return Redirect::to("/entrar?govbr_erro=nonce_mismatch").into_response();
    }
    if claims.aud.as_deref() != Some(cfg.client_id.as_str()) {
        return Redirect::to("/entrar?govbr_erro=aud_mismatch").into_response();
    }

    // 3. Upsert citizen + issue session.
    let session = match upsert_citizen_and_session(&state.db, &claims).await {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(?err, "gov.br upsert falhou");
            return Redirect::to("/entrar?govbr_erro=persist").into_response();
        }
    };

    // 4. Session cookie + clear the gov.br state cookie.
    let session_cookie = format!(
        "dsoc_session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        session.session_id, session.max_age_secs,
    );
    let kill_state = "dsoc_govbr=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0";
    (
        StatusCode::FOUND,
        [
            (header::SET_COOKIE, session_cookie),
            (header::SET_COOKIE, kill_state.to_owned()),
            (header::LOCATION, "/bem-vinda".to_owned()),
        ],
        "",
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiResponse::<()>::fail(
            "govbr_unconfigured",
            "Login gov.br não está habilitado nesta instância.",
        )),
    )
        .into_response()
}

fn read_govbr_cookie(headers: &HeaderMap) -> Option<(String, String)> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for kv in raw.split(';') {
        let kv = kv.trim();
        if let Some(v) = kv.strip_prefix("dsoc_govbr=") {
            let (s, n) = v.split_once('|')?;
            return Some((s.to_owned(), n.to_owned()));
        }
    }
    None
}

fn urlsafe_random(n: usize) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn urlencode(s: &str) -> String {
    // Minimal URL encoder pros chars comuns em client_id / redirect_uri.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Token exchange
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenResp {
    id_token: String,
    #[allow(dead_code)]
    access_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    token_type: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    expires_in: Option<i64>,
}

async fn exchange_code(cfg: &Cfg, code: &str) -> Result<TokenResp, reqwest::Error> {
    let body = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &cfg.redirect_uri),
    ];
    reqwest::Client::new()
        .post(format!("{}/token", cfg.issuer))
        .basic_auth(&cfg.client_id, Some(&cfg.client_secret))
        .form(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<TokenResp>()
        .await
}

// ---------------------------------------------------------------------------
// id_token decode (WITHOUT JWKS verification — TODO next slice)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    aud: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    /// gov.br: array of levels reached ("1", "2", "3" — bronze/silver/gold)
    /// ou strings equivalentes. Pega o mais alto.
    #[serde(default)]
    #[allow(dead_code)]
    amr: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    acr: Option<String>,
    /// Set when the `cpf` scope was requested (not requested at this first stage).
    #[serde(default)]
    cpf: Option<String>,
}

fn decode_id_token(jwt: &str) -> Result<IdClaims, String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err("id_token malformado".to_owned());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| format!("b64: {e}"))?;
    let claims: IdClaims = serde_json::from_slice(&payload).map_err(|e| format!("json: {e}"))?;
    Ok(claims)
}

// ---------------------------------------------------------------------------
// Upsert + session
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct SessionOut {
    session_id: Uuid,
    max_age_secs: i64,
}

async fn upsert_citizen_and_session(
    db: &PgPool,
    claims: &IdClaims,
) -> Result<SessionOut, sqlx::Error> {
    // Assumes a single org (11111...1). A next slice accepts the org from env.
    let org_id: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
    let now = chrono::Utc::now();
    let ttl_secs: i64 = 30 * 24 * 3600;
    let expires_at = now + chrono::Duration::seconds(ttl_secs);

    let mut tx = db.begin().await?;

    // Try to find them by govbr_sub. If found, update legal_name (the name may
    // ter mudado no gov.br entre logins) e devolve o id.
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM citizen WHERE govbr_sub = $1")
        .bind(&claims.sub)
        .fetch_optional(&mut *tx)
        .await?;
    let citizen_id: Uuid = if let Some(id) = existing {
        sqlx::query(
            r"UPDATE citizen
                 SET legal_name = COALESCE($2, legal_name),
                     govbr_confiabilidade = COALESCE($3, govbr_confiabilidade),
                     profile_updated_at = now()
               WHERE id = $1",
        )
        .bind(id)
        .bind(&claims.name)
        .bind::<Option<&str>>(None) // TODO: mapear amr/acr → bronze/prata/ouro
        .execute(&mut *tx)
        .await?;
        id
    } else {
        let id = Uuid::now_v7();
        sqlx::query(
            r"INSERT INTO citizen
                 (id, org_id, oidc_subject, verification_level, is_public,
                  display_name, legal_name, govbr_sub, govbr_linked_at, created_at)
               VALUES ($1, $2, $3, 'directory', true, $4, $4, $5, $6, $6)",
        )
        .bind(id)
        .bind(org_id)
        .bind(format!("govbr:{}", claims.sub))
        .bind(&claims.name)
        .bind(&claims.sub)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        id
    };

    // When an email/document arrives, store it in auth_credential (optional — no password).
    if let Some(email) = &claims.email {
        let email_lc = email.to_lowercase();
        sqlx::query(
            r"INSERT INTO auth_credential
                 (id, citizen_id, org_id, email, password_hash, cpf, cpf_status, created_at)
               VALUES ($1, $2, $3, $4, '', COALESCE($5, ''), 'unverified', now())
               ON CONFLICT (org_id, email) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(citizen_id)
        .bind(org_id)
        .bind(&email_lc)
        .bind(&claims.cpf)
        .execute(&mut *tx)
        .await?;
    }

    // Session
    let session_id = Uuid::now_v7();
    let handle = format!("u-{}", citizen_id.as_simple());
    let subject = format!("govbr:{}", claims.sub);
    sqlx::query(
        r"INSERT INTO auth_session
             (id, org_id, citizen_id, oidc_subject, issued_at, expires_at, public_handle)
           VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(session_id)
    .bind(org_id)
    .bind(citizen_id)
    .bind(&subject)
    .bind(now)
    .bind(expires_at)
    .bind(&handle)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(SessionOut {
        session_id,
        max_age_secs: ttl_secs,
    })
}
