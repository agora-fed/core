//! Completar cadastro obrigatório (ÁGORA, 0664) — para usuários que se cadastraram ANTES de os
//! campos pessoais serem capturados (nome/sexo/nascimento eram descartados; nick era automático).
//!
//! - `GET  /me/profile-status`   — {complete, missing:[...]} — o front usa pra decidir o gate.
//! - `POST /me/complete-profile` — grava nome→display_name/legal_name, sexo→gender, nascimento→
//!   birth_date, e opcionalmente troca o nick (handle) se ainda for o automático `cidadao-*`.
//!
//! Runtime queries (padrão do gateway). CallerId dá a identidade autenticada.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/profile-status", get(status))
        .route("/me/complete-profile", post(complete))
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

/// Mapeia sexo do formulário (F/M) para o vocabulário de `citizen.gender`.
fn sexo_to_gender(sexo: &str) -> Option<&'static str> {
    match sexo.trim().to_uppercase().as_str() {
        "F" => Some("mulher"),
        "M" => Some("homem"),
        _ => None,
    }
}

/// Nick válido: 3–30 chars, `[a-z0-9_]`, começa por letra (bate com o cadastro).
fn valid_handle(h: &str) -> bool {
    let n = h.chars().count();
    (3..=30).contains(&n)
        && h.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[derive(Serialize)]
struct StatusDto {
    complete: bool,
    missing: Vec<&'static str>,
    /// True se o handle ainda é o automático (`cidadao-*`) — o front oferece trocar.
    auto_handle: bool,
}

async fn status(State(state): State<AppState>, caller: CallerId) -> Response {
    let citizen = caller.citizen.as_uuid();
    let row: Result<(Option<String>, Option<String>, Option<chrono::NaiveDate>, Option<String>), _> =
        sqlx::query_as(
            "SELECT NULLIF(display_name,''), gender, birth_date, handle FROM citizen WHERE id = $1",
        )
        .bind(citizen)
        .fetch_one(&state.db)
        .await;
    match row {
        Ok((name, gender, birth, handle)) => {
            let mut missing = Vec::new();
            if name.is_none() {
                missing.push("nome");
            }
            if gender.is_none() {
                missing.push("sexo");
            }
            if birth.is_none() {
                missing.push("nascimento");
            }
            let auto_handle = handle.as_deref().is_none_or(|h| h.starts_with("cidadao-"));
            (
                StatusCode::OK,
                Json(ApiResponse::ok(StatusDto {
                    complete: missing.is_empty(),
                    missing,
                    auto_handle,
                })),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(?err, "profile-status");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct CompleteBody {
    nome: String,
    sexo: String,
    /// `YYYY-MM-DD`.
    nascimento: String,
    /// Novo nick (opcional; só aplica se o atual for automático e este estiver livre).
    #[serde(default)]
    handle: Option<String>,
}

async fn complete(State(state): State<AppState>, caller: CallerId, Json(body): Json<CompleteBody>) -> Response {
    let citizen = caller.citizen.as_uuid();
    let org = caller.org.as_uuid();

    let nome = body.nome.trim();
    if nome.split_whitespace().count() < 2 {
        return fail(StatusCode::BAD_REQUEST, "invalid_name", "Informe nome e sobrenome.");
    }
    let Some(gender) = sexo_to_gender(&body.sexo) else {
        return fail(StatusCode::BAD_REQUEST, "invalid_sexo", "Informe seu sexo.");
    };
    let Ok(birth) = chrono::NaiveDate::parse_from_str(body.nascimento.trim(), "%Y-%m-%d") else {
        return fail(StatusCode::BAD_REQUEST, "invalid_birth", "Data de nascimento inválida.");
    };

    // Troca de nick (opcional): só se o usuário informou um e ele é válido + livre + o atual é auto.
    let new_handle: Option<String> = match body.handle.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => {
            let h = h.trim_start_matches('@').to_lowercase();
            if !valid_handle(&h) {
                return fail(
                    StatusCode::BAD_REQUEST,
                    "invalid_handle",
                    "Nick inválido: 3–30 caracteres (a–z, 0–9, _), começando por letra.",
                );
            }
            let taken: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM citizen WHERE org_id=$1 AND lower(handle)=lower($2) AND id<>$3)",
            )
            .bind(org)
            .bind(&h)
            .bind(citizen)
            .fetch_one(&state.db)
            .await
            .unwrap_or(true);
            if taken {
                return fail(StatusCode::CONFLICT, "handle_taken", "Esse nick já está em uso.");
            }
            Some(h)
        }
        None => None,
    };

    // Atualiza os dados pessoais; o nick só é trocado quando o atual é automático (não sobrescreve
    // um nick já escolhido). CASE mantém o handle atual se não houver troca.
    let res = sqlx::query(
        r"UPDATE citizen SET
            display_name = $2,
            legal_name   = $2,
            gender       = $3,
            birth_date   = $4,
            handle       = CASE WHEN $5::text IS NOT NULL AND handle LIKE 'cidadao-%'
                                THEN $5 ELSE handle END,
            profile_updated_at = now()
          WHERE id = $1",
    )
    .bind(citizen)
    .bind(nome)
    .bind(gender)
    .bind(birth)
    .bind(new_handle.as_deref())
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(serde_json::json!({ "complete": true })))).into_response(),
        Err(err) => {
            tracing::error!(?err, "complete-profile");
            storage_error()
        }
    }
}
