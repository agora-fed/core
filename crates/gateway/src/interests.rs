//! Interesses do cidadão (áreas temáticas ministeriais) — migration 0661.
//!
//! O cidadão marca no perfil quais áreas quer acompanhar (Saúde, Educação, Segurança, Receita
//! Federal, Cultura, Esporte…), baseadas na estrutura ministerial. Alimenta o direcionamento
//! futuro de atualizações/consultas por tema. English identifiers no código; áreas em pt (dados).
//! Runtime queries.
//!
//! - `GET /interest-areas`  — lista PÚBLICA das áreas disponíveis.
//! - `GET /me/interests`    — meus slugs selecionados.
//! - `PUT /me/interests`    — substitui minha seleção.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/interest-areas", get(list_areas))
        .route("/me/interests", get(my_interests).put(set_interests))
        .with_state(state)
}

fn storage_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

#[derive(Serialize)]
struct AreaDto {
    slug: String,
    name: String,
    ministry: Option<String>,
}

async fn list_areas(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<(String, String, Option<String>)>, sqlx::Error> = sqlx::query_as(
        "SELECT slug, name, ministry FROM interest_area ORDER BY position, name",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => {
            let dtos: Vec<AreaDto> = items
                .into_iter()
                .map(|(slug, name, ministry)| AreaDto { slug, name, ministry })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_areas");
            storage_error()
        }
    }
}

async fn my_interests(State(state): State<AppState>, caller: CallerId) -> Response {
    let citizen = caller.citizen.as_uuid();
    let rows: Result<Vec<(String,)>, sqlx::Error> =
        sqlx::query_as("SELECT area_slug FROM citizen_interest WHERE citizen_id = $1")
            .bind(citizen)
            .fetch_all(&state.db)
            .await;
    match rows {
        Ok(items) => {
            let slugs: Vec<String> = items.into_iter().map(|(s,)| s).collect();
            (StatusCode::OK, Json(ApiResponse::ok(slugs))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "my_interests");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct SetBody {
    areas: Vec<String>,
}

async fn set_interests(State(state): State<AppState>, caller: CallerId, Json(body): Json<SetBody>) -> Response {
    let citizen = caller.citizen.as_uuid();
    // Substitui a seleção: apaga tudo e insere só as áreas VÁLIDAS (FK garante existência).
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "set_interests: begin");
            return storage_error();
        }
    };
    if sqlx::query("DELETE FROM citizen_interest WHERE citizen_id = $1")
        .bind(citizen)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return storage_error();
    }
    let mut seen = std::collections::HashSet::new();
    for slug in body.areas.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !seen.insert(slug) {
            continue;
        }
        // A subquery EXISTS + a FK ignoram slug inválido de forma segura.
        if let Err(err) = sqlx::query(
            r"INSERT INTO citizen_interest (citizen_id, area_slug)
              SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM interest_area WHERE slug = $2)
              ON CONFLICT DO NOTHING",
        )
        .bind(citizen)
        .bind(slug)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(?err, "set_interests: insert");
            return storage_error();
        }
    }
    if tx.commit().await.is_err() {
        return storage_error();
    }
    (StatusCode::OK, Json(ApiResponse::ok(serde_json::json!({ "saved": true })))).into_response()
}
