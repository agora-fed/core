//! Estatísticas públicas ao vivo — usadas na landing pra dar peso à tese.
//!
//! `GET /stats/public` — SEM autenticação, cacheável. Retorna:
//! - `citizens_active`: cidadãs(os) verificadas(os) por e-mail (não anônimas).
//! - `proposals_published`: propostas que passaram por moderação e federaram.
//! - `mandates_indexed`: mandatos federais + estaduais catalogados.
//! - `responses_public`: SLAs respondidas dentro do prazo (accountability
//!   demonstrada).
//! - `silences_public`: SLAs vencidas sem resposta (silêncio público).
//! - `response_rate`: % (respostas / (respostas + silêncios)) ou null se sem SLAs.
//!
//! Sem PII. Sem inferência de identidade. Só contadores.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::Serialize;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/stats/public", get(public_stats))
        .with_state(state)
}

#[derive(Debug, Serialize)]
struct PublicStats {
    citizens_active: i64,
    proposals_published: i64,
    mandates_indexed: i64,
    responses_public: i64,
    silences_public: i64,
    response_rate: Option<f64>,
    generated_at: chrono::DateTime<chrono::Utc>,
}

async fn public_stats(State(state): State<AppState>) -> Response {
    let db = &state.db;
    // Uma query por métrica — barato, indexado, sem PII. Tolerante a falha:
    // conta como 0 se der erro pra manter a landing renderizando.
    let citizens_active: i64 = sqlx::query_scalar(
        r"SELECT COUNT(*) FROM citizen
           WHERE verification_level IN ('email','directory','strong')
             AND (deleted_at IS NULL)",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);
    let proposals_published: i64 = sqlx::query_scalar(
        r"SELECT COUNT(*) FROM proposal WHERE status IN ('published','clustered')",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);
    let mandates_indexed: i64 = sqlx::query_scalar(
        r"SELECT COUNT(*) FROM mandate
           WHERE sphere IN ('federal','estadual') AND is_candidate = false",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);
    let responses_public: i64 = sqlx::query_scalar(
        r"SELECT COUNT(*) FROM consequence_sla WHERE status IN ('answered','acted')",
    )
    .fetch_one(db)
    .await
    .unwrap_or(0);
    let silences_public: i64 =
        sqlx::query_scalar(r"SELECT COUNT(*) FROM consequence_sla WHERE status = 'ignored'")
            .fetch_one(db)
            .await
            .unwrap_or(0);
    let response_rate = {
        let denom = responses_public + silences_public;
        if denom > 0 {
            Some((responses_public as f64) * 100.0 / (denom as f64))
        } else {
            None
        }
    };
    let stats = PublicStats {
        citizens_active,
        proposals_published,
        mandates_indexed,
        responses_public,
        silences_public,
        response_rate,
        generated_at: chrono::Utc::now(),
    };
    (StatusCode::OK, Json(ApiResponse::ok(stats))).into_response()
}
