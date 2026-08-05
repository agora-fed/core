//! # Electoral registry — political-citizenship verification (0.25.0).
//!
//! Validates the registry number (12 digits + 2 TSE check digits) via the
//! official algorithm: DV1 = mod 11 of weights 2..=9 over the first 8 digits
//! (with a special rule for SP and MG), DV2 = mod 11 over UF + DV1 with weights
//! 7..=9.
//!
//! `POST /api/v1/me/titulo-eleitor` valida algoritmicamente e grava
//! `titulo_status = 'validated'`. Promotion to `'verified'` (a cross-check with
//! future TSE open data) is left to a later slice.

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
// Registry validator — the official TSE algorithm (l10n_br, ADR-0015)
// ---------------------------------------------------------------------------
// Algorithmic validation of the electoral registry is Brazil-specific code and lives in
// `dsoc_l10n_br::voter` (`BrVoterRegistration`), behind the country-agnostic trait
// `dsoc_core::VoterRegistration`. The `submit` handler below delegates the validation; the
// UX/route stays identical.

// ---------------------------------------------------------------------------
// GET /me/titulo-eleitor
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusDto {
    /// The last 4 digits only (security + LGPD). NULL when not registered.
    titulo_last4: Option<String>,
    /// One of: `unverified` | `validated` | `verified` | (NULL when not registered).
    titulo_status: Option<String>,
    /// Declared electoral zone (up to 4 digits). Auxiliary — it does not validate the registry.
    titulo_zona: Option<String>,
    /// Declared electoral section (up to 4 digits). Auxiliary — it does not validate the registry.
    titulo_secao: Option<String>,
}

async fn get_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    #[allow(clippy::type_complexity)]
    let row: Result<
        Option<(
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )>,
        _,
    > = sqlx::query_as(
        r"SELECT titulo_eleitor, titulo_status, titulo_zona, titulo_secao
                FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some((titulo, status, zona, secao))) => {
            let last4 = titulo
                .as_deref()
                .filter(|s| s.chars().count() >= 4)
                .map(|s| s.chars().skip(s.chars().count() - 4).collect::<String>());
            (
                StatusCode::OK,
                Json(ApiResponse::ok(StatusDto {
                    titulo_last4: last4,
                    titulo_status: status,
                    titulo_zona: zona,
                    titulo_secao: secao,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(StatusDto {
                titulo_last4: None,
                titulo_status: None,
                titulo_zona: None,
                titulo_secao: None,
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
    /// The registry's 12 digits. Optional when the citizen already has a registry linked
    /// and is only updating zone/section.
    #[serde(default)]
    titulo: Option<String>,
    /// Electoral zone (optional, up to 4 digits) — printed on the registry itself.
    #[serde(default)]
    zona: Option<String>,
    /// Electoral section (optional, up to 4 digits) — printed on the registry itself.
    #[serde(default)]
    secao: Option<String>,
}

/// Normalize zone/section: accepts empty (→ None) or 1–4 digits (dots/spaces
/// stripped). `Err` when anything non-digit remains or it exceeds 4.
fn normalize_zona_secao(raw: Option<&str>) -> Result<Option<String>, ()> {
    let Some(raw) = raw else { return Ok(None) };
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    let had_other = raw
        .chars()
        .any(|c| !c.is_ascii_digit() && !c.is_whitespace() && c != '.' && c != '-');
    if had_other || digits.len() > 4 {
        return Err(());
    }
    if digits.is_empty() {
        return Ok(None);
    }
    Ok(Some(digits))
}

async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitReq>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let Ok(zona) = normalize_zona_secao(body.zona.as_deref()) else {
        return bad("Zona inválida — use até 4 dígitos (ex.: 123).");
    };
    let Ok(secao) = normalize_zona_secao(body.secao.as_deref()) else {
        return bad("Seção inválida — use até 4 dígitos (ex.: 45).");
    };
    // No new number → zone/section only; requires an already-linked registry.
    let titulo_input = body
        .titulo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(raw_titulo) = titulo_input else {
        let res = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            r"UPDATE citizen
                 SET titulo_zona = $2, titulo_secao = $3
               WHERE id = $1 AND titulo_eleitor IS NOT NULL
           RETURNING titulo_status, right(titulo_eleitor, 4)",
        )
        .bind(citizen)
        .bind(zona.as_deref())
        .bind(secao.as_deref())
        .fetch_optional(&state.db)
        .await;
        return match res {
            Ok(Some((status, last4))) => (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({
                    "titulo_status": status,
                    "titulo_last4": last4,
                    "titulo_zona": zona,
                    "titulo_secao": secao,
                }))),
            )
                .into_response(),
            Ok(None) => bad("Vincule o número do título antes de salvar zona e seção."),
            Err(err) => {
                tracing::error!(?err, "submit_titulo zona/secao");
                server_error()
            }
        };
    };
    // TSE algorithmic validation behind the dsoc_core::VoterRegistration trait (ADR-0015).
    // The error messages are identical to before (preserved in BrVoterRegistration).
    use dsoc_core::VoterRegistration as _;
    let normalized = match dsoc_l10n_br::BrVoterRegistration.validate(raw_titulo) {
        Ok(n) => n,
        Err(dsoc_core::Error::Validation(msg)) => return bad(&msg),
        Err(err) => {
            tracing::error!(?err, "submit_titulo validate");
            return server_error();
        }
    };
    // Updates; ON CONFLICT via the partial UNIQUE raises a violation → 409.
    let res = sqlx::query(
        r"UPDATE citizen
             SET titulo_eleitor = $2,
                 titulo_status  = 'validated',
                 titulo_zona    = $3,
                 titulo_secao   = $4
           WHERE id = $1",
    )
    .bind(citizen)
    .bind(&normalized)
    .bind(zona.as_deref())
    .bind(secao.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "titulo_status": "validated",
                "titulo_last4": &normalized[8..12],
                "titulo_zona": zona,
                "titulo_secao": secao,
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

    // The tests of the registry's algorithmic validation (normalize/check_digits) migrated with the
    // logic to `dsoc_l10n_br::voter` (ADR-0015). Only the HTTP handler tests remain here.

    #[test]
    fn zona_secao_normalization() {
        assert_eq!(normalize_zona_secao(None), Ok(None));
        assert_eq!(normalize_zona_secao(Some("")), Ok(None));
        assert_eq!(normalize_zona_secao(Some("  ")), Ok(None));
        assert_eq!(normalize_zona_secao(Some("123")), Ok(Some("123".into())));
        assert_eq!(normalize_zona_secao(Some("0045")), Ok(Some("0045".into())));
        assert_eq!(normalize_zona_secao(Some("1.2")), Ok(Some("12".into())));
        assert_eq!(normalize_zona_secao(Some("12345")), Err(()));
        assert_eq!(normalize_zona_secao(Some("12a")), Err(()));
    }
}
