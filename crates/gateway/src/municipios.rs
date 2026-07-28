//! # Referência de municípios IBGE (migration 0651) — selector do cadastro.
//!
//! `GET /municipios?uf=XX` — lista PÚBLICA dos municípios de uma UF (código IBGE
//! + nome), ordenada por nome, para o selector UF→município do cadastro
//! (0652/0653: domicílio obrigatório). Runtime query — sem regen do cache sqlx.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/municipios", get(list))
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    uf: Option<String>,
}

#[derive(Serialize)]
struct MunicipioDto {
    codigo_ibge: i32,
    nome: String,
}

/// `GET /municipios?uf=XX` — municípios de uma UF (código IBGE + nome), por nome.
async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let uf = q.uf.unwrap_or_default().trim().to_uppercase();
    if uf.len() != 2 || !uf.chars().all(|c| c.is_ascii_alphabetic()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_uf",
            "Informe a UF como sigla de 2 letras (ex.: SP).",
        );
    }
    // IBGE atrás da abstração agnóstica dsoc_core::TerritorialProvider (ADR-0015).
    use dsoc_core::TerritorialProvider as _;
    let provider = dsoc_l10n_br::BrTerritorialProvider::new(state.db.clone());
    match provider.municipalities(&uf).await {
        Ok(items) => {
            let dtos: Vec<MunicipioDto> = items
                .into_iter()
                .map(|m| MunicipioDto {
                    codigo_ibge: m.code,
                    nome: m.name,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "municipios list");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                "Erro interno.",
            )
        }
    }
}
