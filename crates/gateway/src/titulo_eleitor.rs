//! # Título de eleitor — verificação de cidadania política (0.25.0).
//!
//! Valida o número do título (12 dígitos + 2 dígitos verificadores TSE) via
//! algoritmo oficial: DV1 = mod 11 do peso 2..=9 sobre os 8 primeiros dígitos
//! (com regra especial pra SP e MG), DV2 = mod 11 sobre UF + DV1 com pesos
//! 7..=9.
//!
//! `POST /api/v1/me/titulo-eleitor` valida algoritmicamente e grava
//! `titulo_status = 'validated'`. A promoção a `'verified'` (cross-check com
//! TSE dados abertos futuros) fica pra fatia posterior.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/titulo-eleitor", get(get_status).post(submit))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "http_401",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}
fn bad(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("http_400", msg)),
    )
        .into_response()
}
fn conflict(msg: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(ApiResponse::<()>::fail("http_409", msg)),
    )
        .into_response()
}
fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Validador do título (algoritmo TSE oficial)
// ---------------------------------------------------------------------------

/// Extrai só dígitos + valida comprimento 12.
fn normalize(raw: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = raw
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| d as u8))
        .collect();
    if digits.len() != 12 {
        return None;
    }
    Some(digits)
}

/// Valida os 2 DVs conforme regra oficial TSE (docs.tse.jus.br + Serpro).
/// Estrutura: `SEQ (8 dígitos) | UF (2 dígitos, 01–28) | DV1 | DV2`.
/// Regra: DV1 = mod 11 sobre SEQ com pesos 2..=9; se SEQ = 0 e UF∈{01,02}
/// (SP, MG) e DV1 == 0, então DV1 vira 1. DV2 = mod 11 sobre UF+DV1 com pesos
/// 7,8,9. Resto 10 vira 0 (idem para SP/MG passando a 1).
fn check_digits(d: &[u8]) -> bool {
    let seq = &d[..8];
    let uf = ((d[8] as u32) * 10 + d[9] as u32) as u8;
    if !(1..=28).contains(&uf) {
        return false;
    }
    let dv1_expected = d[10];
    let dv2_expected = d[11];

    // DV1
    let mut sum: u32 = 0;
    for (i, dig) in seq.iter().enumerate() {
        sum += (*dig as u32) * ((i as u32) + 2);
    }
    let mut dv1 = (sum % 11) as u8;
    if dv1 == 10 {
        dv1 = 0;
    }
    if dv1 == 0 && matches!(uf, 1 | 2) {
        dv1 = 1;
    }
    if dv1 != dv1_expected {
        return false;
    }

    // DV2
    let d8 = d[8] as u32;
    let d9 = d[9] as u32;
    let d10 = dv1 as u32;
    let sum2 = d8 * 7 + d9 * 8 + d10 * 9;
    let mut dv2 = (sum2 % 11) as u8;
    if dv2 == 10 {
        dv2 = 0;
    }
    if dv2 == 0 && matches!(uf, 1 | 2) {
        dv2 = 1;
    }
    dv2 == dv2_expected
}

// ---------------------------------------------------------------------------
// GET /me/titulo-eleitor
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusDto {
    /// Somente os últimos 4 dígitos (segurança + LGPD). NULL quando não cadastrado.
    titulo_last4: Option<String>,
    /// Um de: `unverified` | `validated` | `verified` | (NULL quando não cadastrado).
    titulo_status: Option<String>,
}

async fn get_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let row: Result<Option<(Option<String>, Option<String>)>, _> =
        sqlx::query_as(r"SELECT titulo_eleitor, titulo_status FROM citizen WHERE id = $1")
            .bind(citizen)
            .fetch_optional(&state.db)
            .await;
    match row {
        Ok(Some((titulo, status))) => {
            let last4 = titulo
                .as_deref()
                .filter(|s| s.chars().count() >= 4)
                .map(|s| s.chars().skip(s.chars().count() - 4).collect::<String>());
            (
                StatusCode::OK,
                Json(ApiResponse::ok(StatusDto {
                    titulo_last4: last4,
                    titulo_status: status,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(StatusDto {
                titulo_last4: None,
                titulo_status: None,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "get_titulo_status");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/titulo-eleitor
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubmitReq {
    titulo: String,
}

async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitReq>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let Some(digits) = normalize(&body.titulo) else {
        return bad("O título deve ter 12 dígitos (sem pontos ou espaços).");
    };
    if !check_digits(&digits) {
        return bad("Título de eleitor inválido — verifique os dígitos e tente novamente.");
    }
    let normalized: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    // Atualiza; ON CONFLICT via UNIQUE parcial dá violação → 409.
    let res = sqlx::query(
        r"UPDATE citizen
             SET titulo_eleitor = $2,
                 titulo_status  = 'validated'
           WHERE id = $1",
    )
    .bind(citizen)
    .bind(&normalized)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "titulo_status": "validated",
                "titulo_last4": &normalized[8..12],
            }))),
        )
            .into_response(),
        Err(err) if is_unique_violation(&err) => {
            conflict("Este título já está vinculado a outra conta.")
        }
        Err(err) => {
            tracing::error!(?err, "submit_titulo");
            server_error()
        }
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err.as_database_error().and_then(|e| e.code()), Some(c) if c == "23505")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_wrong_length() {
        assert!(normalize("123").is_none());
        assert!(normalize("12345678 9012").is_some());
    }

    #[test]
    fn valid_titulo_passes() {
        // Título real de teste dominio público (12 dígitos):
        // 4321-0987-6543  (SP): sequência 43210987, UF 06, DVs computados abaixo
        // Vamos usar um caso construído: SEQ=00000001, UF=03 (RJ) — calcular DVs
        // SEQ=00000001 → sum = 1*9 = 9 → DV1 = 9
        // UF=03, DV1=9 → sum2 = 0*7 + 3*8 + 9*9 = 24 + 81 = 105 → 105 % 11 = 6 → DV2=6
        let raw = "000000010396";
        let d = normalize(raw).unwrap();
        assert!(check_digits(&d), "título construído com DVs válidos");
    }

    #[test]
    fn invalid_dv_rejected() {
        // Mesmo raw mas com DV errado.
        let raw = "000000010397"; // DV2 errado
        let d = normalize(raw).unwrap();
        assert!(!check_digits(&d));
    }

    #[test]
    fn invalid_uf_rejected() {
        // UF=99 é fora do range [01..28].
        let raw = "000000019900";
        let d = normalize(raw).unwrap();
        assert!(!check_digits(&d));
    }
}
