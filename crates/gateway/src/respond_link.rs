//! # Reply-to-respond — the office answers without an account (item 3, 0.30.0).
//!
//! The consequence loop's bottleneck is adoption by the official: requiring
//! registration in order to answer is friction that becomes silence. Solution: the e-mails
//! de aviso ao gabinete carregam um link assinado
//! (`/responder/?sla=<id>&t=<hmac>`); quem controla a caixa OFICIAL do
//! mandate (public data from the legislature/electoral authority) answers right on the page, with no
//! login. Possession of the token IS the authorization — the same model as postal registered mail:
//! whoever signs for delivery is whoever holds the address.
//!
//! Token: `hex(hmac_sha256(RESPOND_LINK_SECRET, sla_id))` — deterministic
//! per SLA (the D+1 and D+2 links are the same), with no new table. Without the
//! env var the feature stays dormant: links are not generated and the surface refuses.
//!
//! - `GET  /respond/context?sla&t` — context for the page (title, mandate,
//!   deadline) given a valid token.
//! - `POST /respond {sla_id, token, body, committed}` — registra a resposta
//!   official via `ConsequenceService::respond` (Conflict = the SLA is already resolved;
//!   the public outcome is permanent).

use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::SlaId;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/respond/context", get(context))
        .route("/respond", post(submit))
        .with_state(state)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Deterministic token of the SLA. `None` = the feature is dormant (env absent).
pub(crate) fn respond_token(sla_id: Uuid) -> Option<String> {
    let secret = std::env::var("RESPOND_LINK_SECRET").ok()?;
    if secret.trim().is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(sla_id.as_bytes());
    Some(hex_encode(&mac.finalize().into_bytes()))
}

/// Constant-time verification (`Mac::verify_slice`); recomputing the HMAC
/// evita guardar token em claro em qualquer lugar.
fn token_valid(sla_id: Uuid, presented: &str) -> bool {
    let Ok(secret) = std::env::var("RESPOND_LINK_SECRET") else {
        return false;
    };
    if secret.trim().is_empty() {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(sla_id.as_bytes());
    let Some(raw) = hex_decode(presented) else {
        return false;
    };
    mac.verify_slice(&raw).is_ok()
}

fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "invalid_token",
            "Link de resposta inválido ou recurso desativado.",
        )),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ContextQuery {
    sla: Uuid,
    t: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct RespondContextDto {
    proposal_title: String,
    mandate_display_name: Option<String>,
    due_at: chrono::DateTime<chrono::Utc>,
    status: String,
}

async fn context(State(state): State<AppState>, Query(q): Query<ContextQuery>) -> Response {
    if !token_valid(q.sla, &q.t) {
        return denied();
    }
    let row: Option<RespondContextDto> = sqlx::query_as(
        r"SELECT p.title AS proposal_title,
                 m.display_name AS mandate_display_name,
                 s.due_at,
                 s.status
            FROM consequence_sla s
            JOIN proposal p ON p.id = s.proposal_id
            LEFT JOIN mandate m ON m.id = s.mandate_id
           WHERE s.id = $1",
    )
    .bind(q.sla)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    match row {
        Some(ctx) => (StatusCode::OK, Json(ApiResponse::ok(ctx))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "SLA não encontrado.")),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    sla_id: Uuid,
    token: String,
    body: String,
    #[serde(default)]
    committed: bool,
}

async fn submit(State(state): State<AppState>, Json(req): Json<SubmitBody>) -> Response {
    if !token_valid(req.sla_id, &req.token) {
        return denied();
    }
    let svc = dsoc_consequence::ConsequenceService::from_state(&state);
    match svc
        .respond(SlaId::from_uuid(req.sla_id), &req.body, req.committed)
        .await
    {
        Ok(outcome) => {
            tracing::info!(sla = %req.sla_id, ?outcome, "reply-to-respond: resposta registrada via link");
            (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response()
        }
        Err(dsoc_core::Error::Conflict(_)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::fail(
                "already_resolved",
                "Este prazo já foi resolvido — o desfecho público é permanente.",
            )),
        )
            .into_response(),
        Err(dsoc_core::Error::Validation(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("invalid_body", &msg)),
        )
            .into_response(),
        Err(dsoc_core::Error::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "SLA não encontrado.")),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, sla = %req.sla_id, "reply-to-respond: respond falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_tamper_rejection() {
        std::env::set_var("RESPOND_LINK_SECRET", "segredo-de-teste");
        let sla = Uuid::now_v7();
        let token = respond_token(sla).expect("token com secret setado");
        assert!(token_valid(sla, &token));
        // Token de OUTRO SLA nunca autoriza este.
        let other = respond_token(Uuid::now_v7()).unwrap();
        assert!(!token_valid(sla, &other));
        // A tampered / non-hex token is refused.
        assert!(!token_valid(sla, "zzzz"));
        std::env::remove_var("RESPOND_LINK_SECRET");
    }

    #[test]
    fn hex_helpers_roundtrip() {
        let bytes = [0u8, 15, 255, 128];
        let enc = hex_encode(&bytes);
        assert_eq!(enc, "000fff80");
        assert_eq!(hex_decode(&enc).unwrap(), bytes.to_vec());
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }
}
