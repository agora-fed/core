//! # Reply-to-respond — resposta do gabinete sem conta (item 3, 0.30.0).
//!
//! O gargalo do loop de consequência é a adoção pelo político: exigir
//! cadastro pra responder é atrito que vira silêncio. Solução: os e-mails
//! de aviso ao gabinete carregam um link assinado
//! (`/responder/?sla=<id>&t=<hmac>`); quem controla a caixa OFICIAL do
//! mandato (dado público Câmara/Senado/TSE) responde direto na página, sem
//! login. A posse do token É a autorização — mesmo modelo de um AR postal:
//! quem assina o recebimento é quem detém o endereço.
//!
//! Token: `hex(hmac_sha256(RESPOND_LINK_SECRET, sla_id))` — determinístico
//! por SLA (o link do D+1 e o do D+2 são o mesmo), sem tabela nova. Sem a
//! env o recurso fica dormant: links não são gerados e a superfície nega.
//!
//! - `GET  /respond/context?sla&t` — contexto pra página (título, mandato,
//!   prazo) mediante token válido.
//! - `POST /respond {sla_id, token, body, committed}` — registra a resposta
//!   oficial via `ConsequenceService::respond` (Conflict = SLA já resolvido;
//!   o desfecho público é permanente).

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

/// Token determinístico do SLA. `None` = recurso dormant (env ausente).
pub(crate) fn respond_token(sla_id: Uuid) -> Option<String> {
    let secret = std::env::var("RESPOND_LINK_SECRET").ok()?;
    if secret.trim().is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(sla_id.as_bytes());
    Some(hex_encode(&mac.finalize().into_bytes()))
}

/// Verificação em tempo constante (`Mac::verify_slice`); recomputar o HMAC
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
        // Token adulterado / não-hex é recusado.
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
