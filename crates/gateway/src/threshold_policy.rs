//! # Dynamic threshold proportional to the electorate (item 4, 0.30.1).
//!
//! The author NO LONGER chooses their own proposal's trigger: a middleware
//! no composition root intercepta `POST /api/v1/proposals`, resolve o
//! the target mandate's territory (sphere/uf/municipality), looks up the official
//! oficial TSE (`electorate`, migration 0522) e REESCREVE o campo
//! the body's `threshold` field with `clamp(ceil(fraction × electorate), floor, ceiling)`.
//! Statistical legitimacy: the same relative effort triggers a consequence
//! in Roraima and in Sao Paulo.
//!
//! Config (env): `THRESHOLD_FRACTION` (default 0.0005 = 0,05% do
//! eleitorado), `THRESHOLD_FLOOR` (default 25), `THRESHOLD_CEIL`
//! (default 10_000). Without an electorate row for the territory: floor + a warn
//! (fail-safe — it never blocks the proposal's creation).

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Public preview route: the propose form shows the trigger computed for the
/// territory BEFORE submission — the author understands the rule instead
/// of picking a number.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/threshold-preview", get(preview))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    mandate_id: Uuid,
}

#[derive(Debug, Serialize)]
struct PreviewDto {
    threshold: i64,
    voters: Option<i64>,
    fraction: f64,
}

async fn preview(State(state): State<AppState>, Query(q): Query<PreviewQuery>) -> Response {
    let fraction = env_f64("THRESHOLD_FRACTION", DEFAULT_FRACTION);
    let floor = env_i64("THRESHOLD_FLOOR", DEFAULT_FLOOR);
    let ceil = env_i64("THRESHOLD_CEIL", DEFAULT_CEIL);
    let voters = voters_for_mandate(&state.db, q.mandate_id).await;
    let threshold = compute_threshold(voters, fraction, floor, ceil);
    (
        StatusCode::OK,
        Json(ApiResponse::ok(PreviewDto {
            threshold,
            voters,
            fraction,
        })),
    )
        .into_response()
}

const DEFAULT_FRACTION: f64 = 0.0005;
const DEFAULT_FLOOR: i64 = 25;
const DEFAULT_CEIL: i64 = 10_000;
const BODY_LIMIT: usize = 64 * 1024;

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| *v > 0.0 && *v <= 1.0)
        .unwrap_or(default)
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &i64| *v > 0)
        .unwrap_or(default)
}

/// The arithmetic itself — pure and testable. It now merely delegates to the shared
/// primitive `dsoc_core::threshold::proportional_threshold`: forums and proposals use the
/// SAME yardstick (D3 of the critique plan). `voters = None` falls back to the floor.
fn compute_threshold(voters: Option<i64>, fraction: f64, floor: i64, ceil: i64) -> i64 {
    dsoc_core::proportional_threshold(voters, fraction, floor, ceil)
}

/// Electorate of the mandate's territory: municipal → municipality; federal and
/// state → UF; without a UF (e.g. president) → national ('BR').
async fn voters_for_mandate(db: &PgPool, mandate_id: Uuid) -> Option<i64> {
    let mandate: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as(r"SELECT sphere, uf, municipio FROM mandate WHERE id = $1")
            .bind(mandate_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let (sphere, uf, municipio) = mandate?;
    let (uf_key, mun_key): (String, Option<String>) = match (sphere.as_str(), uf, municipio) {
        ("municipal", Some(uf), Some(mun)) => (uf, Some(mun)),
        (_, Some(uf), _) => (uf, None),
        (_, None, _) => ("BR".to_owned(), None),
    };
    sqlx::query_scalar(
        r"SELECT voters FROM electorate
           WHERE uf = $1 AND COALESCE(municipio, '') = COALESCE($2, '')",
    )
    .bind(uf_key)
    .bind(mun_key)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

/// Middleware: rewrites `threshold` on proposal create. Any parse failure
/// lets the request through untouched — the handler validates as always.
pub async fn threshold_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let is_create = req.method() == axum::http::Method::POST
        && req
            .uri()
            .path()
            .trim_end_matches('/')
            .ends_with("/proposals");
    if !is_create {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => {
            return (
                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                "payload grande demais",
            )
                .into_response();
        }
    };
    let rewritten = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(mut json) => {
            let mandate_id = json
                .get("mandate_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<Uuid>().ok());
            if let (Some(obj), Some(mandate_id)) = (json.as_object_mut(), mandate_id) {
                let fraction = env_f64("THRESHOLD_FRACTION", DEFAULT_FRACTION);
                let floor = env_i64("THRESHOLD_FLOOR", DEFAULT_FLOOR);
                let ceil = env_i64("THRESHOLD_CEIL", DEFAULT_CEIL);
                let voters = voters_for_mandate(&state.db, mandate_id).await;
                if voters.is_none() {
                    tracing::warn!(%mandate_id, "threshold policy: sem eleitorado; usando piso");
                }
                let threshold = compute_threshold(voters, fraction, floor, ceil);
                obj.insert("threshold".into(), serde_json::json!(threshold));
                tracing::info!(%mandate_id, threshold, ?voters, "threshold policy applied");
                serde_json::to_vec(&json).ok()
            } else {
                None
            }
        }
        Err(_) => None,
    };
    let final_bytes = rewritten.map(axum::body::Bytes::from).unwrap_or(bytes);
    next.run(Request::from_parts(parts, Body::from(final_bytes)))
        .await
}

#[cfg(test)]
mod tests {
    use super::compute_threshold;

    #[test]
    fn threshold_scales_with_electorate_and_clamps() {
        // 0.05% of 1M voters = 500.
        assert_eq!(compute_threshold(Some(1_000_000), 0.0005, 25, 10_000), 500);
        // A small municipality falls back to the floor (0.05% of 8k = 4 → 25).
        assert_eq!(compute_threshold(Some(8_000), 0.0005, 25, 10_000), 25);
        // National hits the ceiling (0.05% of 155M = 77,500 → 10,000).
        assert_eq!(
            compute_threshold(Some(155_000_000), 0.0005, 25, 10_000),
            10_000
        );
        // No territory data: floor, never blocks.
        assert_eq!(compute_threshold(None, 0.0005, 25, 10_000), 25);
        // Rounds up: 0.05% of 50,001 = 25.0005 → 26.
        assert_eq!(compute_threshold(Some(50_001), 0.0005, 25, 10_000), 26);
    }
}
