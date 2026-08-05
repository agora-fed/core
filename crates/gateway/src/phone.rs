//! `/me/phone` — the citizen's phone + OTP SMS verification (AGORA F5, #62, migration 0658).
//!
//! **Opt-in.** The citizen supplies the phone; we send a 6-digit code by SMS (via
//! INTERCOMS/`SmsGatewayProvider`, ADR-0016) and they confirm. We store only the code's SHA-256
//! (TTL 10 min). It enables SMS 2FA (not recommended — an alternative for e-mail loss) and the
//! SMS reach. English API, runtime queries.
//!
//! - `GET  /me/phone`        — my phone + whether it is verified.
//! - `POST /me/phone`        — define o telefone e dispara o OTP.
//! - `POST /me/phone/verify` — confirm the code.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::intercoms::{MessageSender, OutboundMessage, SmsGatewayProvider};

const OTP_TTL_MINUTES: i64 = 10;
const RESEND_COOLDOWN_SECS: i64 = 60;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/phone", get(status).post(set_phone))
        .route("/me/phone/verify", post(verify))
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

/// Normalize to something E.164-ish (+ and digits). Loose: it does not validate the carrier.
fn normalize_phone(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for (i, c) in raw.trim().chars().enumerate() {
        if c == '+' && i == 0 {
            out.push('+');
        } else if c.is_ascii_digit() {
            out.push(c);
        }
    }
    let digits = out.trim_start_matches('+').len();
    if (8..=15).contains(&digits) {
        Some(if out.starts_with('+') {
            out
        } else {
            format!("+{out}")
        })
    } else {
        None
    }
}

fn hash_code(code: &str) -> Vec<u8> {
    Sha256::digest(code.as_bytes()).to_vec()
}

#[derive(Serialize)]
struct PhoneStatus {
    phone: Option<String>,
    verified: bool,
}

async fn status(State(state): State<AppState>, caller: CallerId) -> Response {
    let citizen = caller.citizen.as_uuid();
    let row: Result<(Option<String>, Option<chrono::DateTime<chrono::Utc>>), sqlx::Error> =
        sqlx::query_as("SELECT phone, phone_verified_at FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_one(&state.db)
            .await;
    match row {
        Ok((phone, verified_at)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(PhoneStatus {
                phone,
                verified: verified_at.is_some(),
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "phone status");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct SetPhoneBody {
    phone: String,
}

async fn set_phone(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<SetPhoneBody>,
) -> Response {
    let citizen = caller.citizen.as_uuid();
    let Some(phone) = normalize_phone(&body.phone) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_phone",
            "Telefone inválido (use DDD e número).",
        );
    };
    // Resend cooldown.
    let recent: bool = match sqlx::query_scalar(
        r"SELECT EXISTS(SELECT 1 FROM phone_otp
                         WHERE citizen_id = $1 AND created_at > now() - ($2 || ' seconds')::interval)",
    )
    .bind(citizen)
    .bind(RESEND_COOLDOWN_SECS.to_string())
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "phone: cooldown");
            return storage_error();
        }
    };
    if recent {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "cooldown",
            "Aguarde um minuto para pedir outro código.",
        );
    }

    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    let code_hash = hash_code(&code);
    if let Err(err) = sqlx::query(
        r"INSERT INTO phone_otp (citizen_id, phone, code_hash, expires_at)
          VALUES ($1, $2, $3, now() + ($4 || ' minutes')::interval)",
    )
    .bind(citizen)
    .bind(&phone)
    .bind(&code_hash)
    .bind(OTP_TTL_MINUTES.to_string())
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "phone: insert otp");
        return storage_error();
    }

    // Send via INTERCOMS. Without an SMSGateway configured it only logs (dev / not provisioned).
    if let Some(sender) = SmsGatewayProvider::from_env() {
        let msg = OutboundMessage {
            channel: crate::intercoms::Channel::Sms,
            to: phone.clone(),
            subject: String::new(),
            body: format!(
                "DemocraciaBR: seu codigo de verificacao e {code}. Vale {OTP_TTL_MINUTES} min."
            ),
        };
        if let Err(err) = sender.send(&msg).await {
            tracing::warn!(?err, "phone: envio de OTP falhou");
            return fail(
                StatusCode::BAD_GATEWAY,
                "sms_failed",
                "Não foi possível enviar o SMS agora.",
            );
        }
    } else {
        tracing::warn!(%phone, "phone: SMSGateway não configurado — OTP não enviado");
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "sms_unconfigured",
            "Verificação por SMS ainda não está disponível nesta instância.",
        );
    }

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "sent": true }))),
    )
        .into_response()
}

#[derive(Deserialize)]
struct VerifyBody {
    code: String,
}

async fn verify(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<VerifyBody>,
) -> Response {
    let citizen = caller.citizen.as_uuid();
    let code = body.code.trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_code",
            "Código de 6 dígitos.",
        );
    }
    // The most recent OTP, alive and unused.
    let row: Result<Option<(uuid::Uuid, String, Vec<u8>)>, sqlx::Error> = sqlx::query_as(
        r"SELECT id, phone, code_hash FROM phone_otp
           WHERE citizen_id = $1 AND used_at IS NULL AND expires_at > now()
           ORDER BY created_at DESC LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await;
    let (otp_id, phone, code_hash) = match row {
        Ok(Some(t)) => t,
        Ok(None) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "no_pending",
                "Nenhum código pendente. Peça um novo.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "phone verify: lookup");
            return storage_error();
        }
    };
    if hash_code(code) != code_hash {
        return fail(StatusCode::BAD_REQUEST, "wrong_code", "Código incorreto.");
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "phone verify: begin");
            return storage_error();
        }
    };
    if let Err(err) = sqlx::query("UPDATE phone_otp SET used_at = now() WHERE id = $1")
        .bind(otp_id)
        .execute(&mut *tx)
        .await
    {
        tracing::error!(?err, "phone verify: mark used");
        return storage_error();
    }
    if let Err(err) =
        sqlx::query("UPDATE citizen SET phone = $2, phone_verified_at = now() WHERE id = $1")
            .bind(citizen)
            .bind(&phone)
            .execute(&mut *tx)
            .await
    {
        tracing::error!(?err, "phone verify: set phone");
        return storage_error();
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "phone verify: commit");
        return storage_error();
    }

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "verified": true }))),
    )
        .into_response()
}
