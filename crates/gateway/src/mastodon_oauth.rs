//! # OAuth2 backing store for the Mastodon Client API (0.19.0, migration 0409).
//!
//! Three tables:
//! * `oauth_application` — one row per registered client. `client_secret` is
//!   only returned once (at register time); the DB stores its SHA-256 hex.
//! * `oauth_authorization_code` — 10-minute-TTL codes issued from
//!   /oauth/authorize, exchanged at /oauth/token.
//! * `oauth_access_token` — bearer tokens (also stored as hex hash). Long
//!   default TTL to match our cookie session (30 days).
//!
//! Every function here is runtime-unchecked `sqlx::query*` to keep the
//! `.sqlx/` offline cache stable.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Access-token TTL (30 days). Mirrors the cookie session TTL — matches
/// Mastodon's own default.
pub const TOKEN_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// Authorization-code TTL (10 minutes) per OAuth 2.0 recommendation.
pub const CODE_TTL_SECS: i64 = 10 * 60;

/// One-shot registration result. `client_secret` and (when applicable)
/// `access_token` are returned in cleartext ONCE and then never again.
#[derive(Debug, Clone, Serialize)]
pub struct AppRegistration {
    pub id: Uuid,
    pub client_id: String,
    pub client_secret: String,
    pub name: String,
    pub website: Option<String>,
    pub redirect_uris: Vec<String>,
    pub scopes: String,
    pub vapid_key: Option<String>,
}

/// Token payload returned by /oauth/token.
#[derive(Debug, Clone, Serialize)]
pub struct TokenPayload {
    pub access_token: String,
    pub token_type: &'static str,
    pub scope: String,
    pub created_at: i64,
}

/// Bearer token resolved by the auth middleware. `citizen_id` is `None` for
/// client_credentials tokens (app-only, no user).
#[derive(Debug, Clone)]
pub struct ResolvedToken {
    pub application_id: Uuid,
    pub citizen_id: Option<Uuid>,
    pub scopes: String,
}

/// Errors the OAuth flow can return. Each maps to a Mastodon-compatible
/// {error, error_description} envelope in the HTTP layer.
#[derive(Debug)]
pub enum OAuthError {
    InvalidClient,
    InvalidGrant,
    UnsupportedGrant,
    InvalidRequest(&'static str),
    InvalidCredentials,
    ServerError,
}

impl OAuthError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant | Self::InvalidCredentials => "invalid_grant",
            Self::UnsupportedGrant => "unsupported_grant_type",
            Self::InvalidRequest(_) => "invalid_request",
            Self::ServerError => "server_error",
        }
    }
    pub fn description(&self) -> &str {
        match self {
            Self::InvalidClient => "client_id or client_secret is wrong",
            Self::InvalidGrant => "authorization grant not accepted",
            Self::UnsupportedGrant => "grant_type is not supported",
            Self::InvalidRequest(m) => m,
            Self::InvalidCredentials => "e-mail or password rejected",
            Self::ServerError => "internal error",
        }
    }
}

/// Register a new client. Returns the row + the cleartext secret. Idempotent
/// only insofar as `client_id` is fresh on each call — Mastodon apps must
/// re-register to get a new pair.
pub async fn register_application(
    db: &PgPool,
    name: &str,
    redirect_uris: Vec<String>,
    scopes: &str,
    website: Option<&str>,
) -> Result<AppRegistration, sqlx::Error> {
    let id = Uuid::now_v7();
    let client_id = random_token(22);
    let client_secret = random_token(43);
    let secret_hash = hash_hex(&client_secret);
    let scopes_stored = normalize_scopes(scopes);
    let now = Utc::now();
    sqlx::query(
        r"INSERT INTO oauth_application
             (id, client_id, client_secret_hash, name, redirect_uris,
              scopes, website, created_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(&client_id)
    .bind(&secret_hash)
    .bind(name)
    .bind(&redirect_uris)
    .bind(&scopes_stored)
    .bind(website)
    .bind(now)
    .execute(db)
    .await?;
    Ok(AppRegistration {
        id,
        client_id,
        client_secret,
        name: name.to_owned(),
        website: website.map(str::to_owned),
        redirect_uris,
        scopes: scopes_stored,
        vapid_key: None,
    })
}

/// Look up an application by its public client_id.
pub async fn find_application_by_client_id(
    db: &PgPool,
    client_id: &str,
) -> Result<Option<(Uuid, String, Vec<String>, String)>, sqlx::Error> {
    // (id, client_secret_hash, redirect_uris, scopes)
    sqlx::query_as::<_, (Uuid, String, Vec<String>, String)>(
        r"SELECT id, client_secret_hash, redirect_uris, scopes
            FROM oauth_application
           WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(db)
    .await
}

/// Constant-time compare a submitted `client_secret` against the stored hash.
#[must_use]
pub fn verify_client_secret(submitted: &str, stored_hash: &str) -> bool {
    let submitted_hash = hash_hex(submitted);
    constant_time_eq(submitted_hash.as_bytes(), stored_hash.as_bytes())
}

/// Issue an access token (client_credentials or password grant path). Called
/// after the caller has been authenticated by the specific grant flow.
pub async fn issue_access_token(
    db: &PgPool,
    application_id: Uuid,
    citizen_id: Option<Uuid>,
    scopes: &str,
) -> Result<TokenPayload, sqlx::Error> {
    let token = random_token(43);
    let hash = hash_hex(&token);
    let now = Utc::now();
    let expires = now + Duration::seconds(TOKEN_TTL_SECS);
    sqlx::query(
        r"INSERT INTO oauth_access_token
             (id, application_id, citizen_id, token_hash, scopes,
              expires_at, revoked_at, created_at)
          VALUES ($1, $2, $3, $4, $5, $6, NULL, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(application_id)
    .bind(citizen_id)
    .bind(&hash)
    .bind(scopes)
    .bind(expires)
    .bind(now)
    .execute(db)
    .await?;
    Ok(TokenPayload {
        access_token: token,
        token_type: "Bearer",
        scope: scopes.to_owned(),
        created_at: now.timestamp(),
    })
}

/// Issue a fresh authorization code for the browser-facing /oauth/authorize
/// step. Consumed by /oauth/token with grant_type=authorization_code.
pub async fn issue_authorization_code(
    db: &PgPool,
    application_id: Uuid,
    citizen_id: Uuid,
    redirect_uri: &str,
    scopes: &str,
) -> Result<String, sqlx::Error> {
    let code = random_token(43);
    let hash = hash_hex(&code);
    let now = Utc::now();
    let expires = now + Duration::seconds(CODE_TTL_SECS);
    sqlx::query(
        r"INSERT INTO oauth_authorization_code
             (id, application_id, citizen_id, code_hash, redirect_uri,
              scopes, expires_at, used_at, created_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(application_id)
    .bind(citizen_id)
    .bind(&hash)
    .bind(redirect_uri)
    .bind(scopes)
    .bind(expires)
    .bind(now)
    .execute(db)
    .await?;
    Ok(code)
}

/// Exchange an authorization code for an access token. Enforces:
/// * code exists AND not used AND not expired
/// * application_id matches the client_id
/// * redirect_uri matches
/// Marks the code used before issuing (atomic-ish; a race would still
/// double-issue, but the second grant would see used_at set and 400).
pub async fn exchange_authorization_code(
    db: &PgPool,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenPayload, OAuthError> {
    let app = find_application_by_client_id(db, client_id)
        .await
        .map_err(|_| OAuthError::ServerError)?
        .ok_or(OAuthError::InvalidClient)?;
    let (app_id, secret_hash, _uris, _scopes) = app;
    if !verify_client_secret(client_secret, &secret_hash) {
        return Err(OAuthError::InvalidClient);
    }
    let code_hash = hash_hex(code);
    let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>)> =
        sqlx::query_as(
            r"SELECT application_id, citizen_id, redirect_uri, scopes, expires_at, used_at
                FROM oauth_authorization_code
               WHERE code_hash = $1",
        )
        .bind(&code_hash)
        .fetch_optional(db)
        .await
        .map_err(|_| OAuthError::ServerError)?;
    let (row_app, citizen_id, row_redirect, scopes, expires_at, used_at) =
        row.ok_or(OAuthError::InvalidGrant)?;
    if row_app != app_id
        || row_redirect != redirect_uri
        || used_at.is_some()
        || expires_at < Utc::now()
    {
        return Err(OAuthError::InvalidGrant);
    }
    // Mark used before issuing so a replay is caught.
    sqlx::query(
        r"UPDATE oauth_authorization_code SET used_at = now() WHERE code_hash = $1",
    )
    .bind(&code_hash)
    .execute(db)
    .await
    .map_err(|_| OAuthError::ServerError)?;
    issue_access_token(db, app_id, Some(citizen_id), &scopes)
        .await
        .map_err(|_| OAuthError::ServerError)
}

/// Issue a token via the password grant. Verifies client + credentials, then
/// stamps a token. Same TTL as the code flow.
pub async fn exchange_password(
    db: &PgPool,
    client_id: &str,
    client_secret: &str,
    citizen_id: Uuid,
    scopes: &str,
) -> Result<TokenPayload, OAuthError> {
    let app = find_application_by_client_id(db, client_id)
        .await
        .map_err(|_| OAuthError::ServerError)?
        .ok_or(OAuthError::InvalidClient)?;
    let (app_id, secret_hash, _uris, _stored_scopes) = app;
    if !verify_client_secret(client_secret, &secret_hash) {
        return Err(OAuthError::InvalidClient);
    }
    issue_access_token(db, app_id, Some(citizen_id), scopes)
        .await
        .map_err(|_| OAuthError::ServerError)
}

/// Look up a bearer token. Returns (application_id, citizen_id, scopes) when
/// the token is valid + unrevoked + not expired; `None` otherwise.
pub async fn resolve_bearer(
    db: &PgPool,
    token: &str,
) -> Result<Option<ResolvedToken>, sqlx::Error> {
    if token.is_empty() {
        return Ok(None);
    }
    let hash = hash_hex(token);
    sqlx::query_as::<_, (Uuid, Option<Uuid>, String, DateTime<Utc>)>(
        r"SELECT application_id, citizen_id, scopes, expires_at
            FROM oauth_access_token
           WHERE token_hash = $1
             AND revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await
    .map(|opt| {
        opt.and_then(|(app, citizen, scopes, exp)| {
            if exp < Utc::now() {
                None
            } else {
                Some(ResolvedToken {
                    application_id: app,
                    citizen_id: citizen,
                    scopes,
                })
            }
        })
    })
}

/// Revoke a bearer token (called from `/oauth/revoke`). Idempotent: revoking
/// a token that doesn't exist is a no-op.
pub async fn revoke_bearer(db: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    let hash = hash_hex(token);
    sqlx::query(
        r"UPDATE oauth_access_token SET revoked_at = now()
           WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn random_token(chars: usize) -> String {
    // 32 random bytes → 43-char base64 URL-no-pad. Trimmed at `chars`.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let enc = URL_SAFE_NO_PAD.encode(bytes);
    enc.chars().take(chars).collect()
}

fn hash_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex_of(&hasher.finalize())
}

fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

fn normalize_scopes(s: &str) -> String {
    normalize_scopes_str(s)
}

/// Public helper — same behaviour, accessible from `mastodon_api.rs` (which
/// needs it when the browser consent flow re-normalizes the query string).
#[must_use]
pub fn normalize_scopes_str(s: &str) -> String {
    let mut parts: Vec<&str> = s
        .split_whitespace()
        .filter(|s| matches!(*s, "read" | "write" | "follow" | "push"))
        .collect();
    parts.sort();
    parts.dedup();
    if parts.is_empty() {
        "read".to_owned()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn random_tokens_are_url_safe_and_sized() {
        let t = random_token(43);
        assert_eq!(t.len(), 43);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn hash_hex_stable() {
        let a = hash_hex("hello");
        let b = hash_hex("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // sha256 hex
    }

    #[test]
    fn ct_eq_rejects_mismatched_len_and_content() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn scope_normalization_trims_and_dedupes() {
        assert_eq!(normalize_scopes(""), "read");
        assert_eq!(normalize_scopes("write  read  garbage"), "read write");
        assert_eq!(normalize_scopes("push write"), "push write");
    }
}
