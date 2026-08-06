//! `/me/2fa/totp` — the citizen's TOTP 2FA (RFC 6238) (AGORA F6, #63, migration 0659).
//!
//! TOTP is the **recommended** 2FA (authenticator app), opt-in. Own implementation (HOTP RFC 4226 +
//! HMAC-SHA1, compatible with Google Authenticator) — no heavy dependency. The base32 secret lives in
//! `citizen.totp_secret`; recovery codes (hashes only) in `totp_recovery_code`. F6.1 =
//! enrollment/management; enforcing it at login is the next slice (F6.2). English API, runtime queries.
//!
//! - `GET  /me/2fa/totp`         — status {enabled}.
//! - `POST /me/2fa/totp/setup`   — gera o segredo (pendente) e devolve {secret, uri}.
//! - `POST /me/2fa/totp/enable`  — confirm a code → enable + return the recovery codes (once).
//! - `POST /me/2fa/totp/disable` — confirm a code → disable and wipe everything.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

const ISSUER: &str = "DemocraciaBR";
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const RECOVERY_COUNT: usize = 8;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/2fa/totp", get(status))
        .route("/me/2fa/totp/setup", post(setup))
        .route("/me/2fa/totp/enable", post(enable))
        .route("/me/2fa/totp/disable", post(disable))
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

// --- base32 (RFC 4648, no padding) ---
fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for c in s.trim().chars().filter(|c| *c != '=') {
        let v = B32
            .iter()
            .position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

// --- HOTP (RFC 4226) / TOTP (RFC 6238) ---
fn hotp(secret: &[u8], counter: u64) -> u32 {
    // HMAC-SHA1 accepts a key of any length: `new_from_slice` never fails here.
    // Even so, we handle it instead of `expect` (clippy::expect_used in the CI gate).
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else {
        return 0;
    };
    mac.update(&counter.to_be_bytes());
    let r = mac.finalize().into_bytes();
    let offset = (r[19] & 0x0f) as usize;
    let bin = ((r[offset] as u32 & 0x7f) << 24)
        | ((r[offset + 1] as u32) << 16)
        | ((r[offset + 2] as u32) << 8)
        | (r[offset + 3] as u32);
    bin % 1_000_000
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Verify a 6-digit code against the current window ±1 (clock tolerance).
fn verify_totp(secret_b32: &str, code: u32) -> bool {
    let Some(secret) = base32_decode(secret_b32) else {
        return false;
    };
    let c = now_secs() / 30;
    [c.wrapping_sub(1), c, c + 1]
        .iter()
        .any(|&cc| hotp(&secret, cc) == code)
}

fn hash(s: &str) -> Vec<u8> {
    Sha256::digest(s.as_bytes()).to_vec()
}

// ---------------------------------------------------------------------------

async fn status(State(state): State<AppState>, caller: CallerId) -> Response {
    let citizen = caller.citizen.as_uuid();
    let enabled: Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> =
        sqlx::query_scalar("SELECT totp_enabled_at FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_one(&state.db)
            .await;
    match enabled {
        Ok(v) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "enabled": v.is_some() }),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "totp status");
            storage_error()
        }
    }
}

#[derive(Serialize)]
struct SetupResult {
    secret: String,
    uri: String,
}

async fn setup(State(state): State<AppState>, caller: CallerId) -> Response {
    let citizen = caller.citizen.as_uuid();
    // Already enabled? Do not overwrite without disabling first.
    let row: Result<(Option<chrono::DateTime<chrono::Utc>>, Option<String>), sqlx::Error> =
        sqlx::query_as("SELECT totp_enabled_at, handle FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_one(&state.db)
            .await;
    let (enabled_at, handle) = match row {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "totp setup: load");
            return storage_error();
        }
    };
    if enabled_at.is_some() {
        return fail(
            StatusCode::CONFLICT,
            "already_enabled",
            "2FA já está ativo. Desative antes de reconfigurar.",
        );
    }
    let mut raw = [0u8; 20];
    rand::thread_rng().fill(&mut raw);
    let secret = base32_encode(&raw);
    // Encrypted at rest (0682/#15): in cleartext this column was a permanent 2FA
    // bypass for anyone holding a dump. No key ⇒ refuse; a cleartext fallback is how
    // a column ends up half protected while everyone believes otherwise.
    let Ok(pii_key) = crate::pii::key() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_pii_key",
            "Proteção de dados pessoais não configurada nesta instância.",
        );
    };
    if let Err(err) =
        sqlx::query("UPDATE citizen SET totp_secret_enc = pgp_sym_encrypt($2, $3) WHERE id = $1")
            .bind(citizen)
            .bind(&secret)
            .bind(&pii_key)
            .execute(&state.db)
            .await
    {
        tracing::error!(?err, "totp setup: store secret");
        return storage_error();
    }
    let label = handle.unwrap_or_else(|| "conta".to_owned());
    let uri = format!(
        "otpauth://totp/{ISSUER}:{label}?secret={secret}&issuer={ISSUER}&algorithm=SHA1&digits=6&period=30"
    );
    (
        StatusCode::OK,
        Json(ApiResponse::ok(SetupResult { secret, uri })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

fn parse_code(s: &str) -> Option<u32> {
    let c = s.trim();
    if c.len() == 6 && c.chars().all(|ch| ch.is_ascii_digit()) {
        c.parse().ok()
    } else {
        None
    }
}

async fn enable(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<CodeBody>,
) -> Response {
    let citizen = caller.citizen.as_uuid();
    let Some(code) = parse_code(&body.code) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_code",
            "Código de 6 dígitos.",
        );
    };
    let Ok(pii_key) = crate::pii::key() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_pii_key",
            "Proteção de dados pessoais não configurada nesta instância.",
        );
    };
    let secret: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT pgp_sym_decrypt(totp_secret_enc, $2) FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .bind(&pii_key)
    .fetch_one(&state.db)
    .await;
    let secret = match secret {
        Ok(Some(s)) => s,
        Ok(None) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "no_setup",
                "Faça o setup primeiro.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "totp enable: load");
            return storage_error();
        }
    };
    if !verify_totp(&secret, code) {
        return fail(
            StatusCode::BAD_REQUEST,
            "wrong_code",
            "Código incorreto. Confira o app.",
        );
    }
    // Enable + generate recovery codes (shown once).
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "totp enable: begin");
            return storage_error();
        }
    };
    if let Err(err) = sqlx::query("UPDATE citizen SET totp_enabled_at = now() WHERE id = $1")
        .bind(citizen)
        .execute(&mut *tx)
        .await
    {
        tracing::error!(?err, "totp enable: set enabled");
        return storage_error();
    }
    // Limpa quaisquer recovery codes antigos e gera novos.
    let _ = sqlx::query("DELETE FROM totp_recovery_code WHERE citizen_id = $1")
        .bind(citizen)
        .execute(&mut *tx)
        .await;
    let mut codes = Vec::with_capacity(RECOVERY_COUNT);
    for _ in 0..RECOVERY_COUNT {
        let n: u64 = rand::thread_rng().gen_range(0..100_000_000);
        let c = format!("{:04}-{:04}", n / 10_000, n % 10_000);
        if let Err(err) =
            sqlx::query("INSERT INTO totp_recovery_code (citizen_id, code_hash) VALUES ($1, $2)")
                .bind(citizen)
                .bind(hash(&c))
                .execute(&mut *tx)
                .await
        {
            tracing::error!(?err, "totp enable: recovery");
            return storage_error();
        }
        codes.push(c);
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "totp enable: commit");
        return storage_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "enabled": true, "recovery_codes": codes }),
        )),
    )
        .into_response()
}

async fn disable(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<CodeBody>,
) -> Response {
    let citizen = caller.citizen.as_uuid();
    let Some(code) = parse_code(&body.code) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_code",
            "Código de 6 dígitos.",
        );
    };
    let Ok(pii_key) = crate::pii::key() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_pii_key",
            "Proteção de dados pessoais não configurada nesta instância.",
        );
    };
    let secret: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT pgp_sym_decrypt(totp_secret_enc, $2) FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .bind(&pii_key)
    .fetch_one(&state.db)
    .await;
    let secret = match secret {
        Ok(Some(s)) => s,
        Ok(None) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "not_enabled",
                "2FA não está ativo.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "totp disable: load");
            return storage_error();
        }
    };
    if !verify_totp(&secret, code) {
        return fail(StatusCode::BAD_REQUEST, "wrong_code", "Código incorreto.");
    }
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return storage_error(),
    };
    let _ = sqlx::query(
        "UPDATE citizen SET totp_secret_enc = NULL, totp_enabled_at = NULL WHERE id = $1",
    )
    .bind(citizen)
    .execute(&mut *tx)
    .await;
    let _ = sqlx::query("DELETE FROM totp_recovery_code WHERE citizen_id = $1")
        .bind(citizen)
        .execute(&mut *tx)
        .await;
    if tx.commit().await.is_err() {
        return storage_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "enabled": false }))),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vetores oficiais RFC 4226 (HOTP), segredo ASCII "12345678901234567890".
    #[test]
    fn hotp_rfc4226_vectors() {
        let secret = b"12345678901234567890";
        let expected = [
            755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (counter, exp) in expected.iter().enumerate() {
            assert_eq!(hotp(secret, counter as u64), *exp, "counter {counter}");
        }
    }

    #[test]
    fn base32_roundtrip() {
        let data = b"12345678901234567890";
        let enc = base32_encode(data);
        assert_eq!(base32_decode(&enc).unwrap(), data);
    }
}
