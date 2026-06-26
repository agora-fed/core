//! # dsoc-gateway — public API surface
//!
//! Assembles `routes(AppState)` from every crate into one Axum router (the strangler-fig front
//! door, PLAN.md section 8). Owns no domain tables; it composes the crate routers, serves the
//! OpenAPI contract, and exposes health. IPv6-first (PLAN.md principle 4).

#![forbid(unsafe_code)]

pub mod worker;

use axum::{routing::get, Json, Router};
use dsoc_app::AppState;
use tower_http::services::ServeDir;

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "dsoc-gateway" }))
}

async fn openapi() -> Json<serde_json::Value> {
    let doc: serde_json::Value =
        serde_json::from_str(&dsoc_api_contract::openapi_json()).unwrap_or_default();
    Json(doc)
}

/// Compose every crate's `routes(state)` under `/api/v1`. Each crate owns its own paths and already
/// carries its state, so the gateway merges them; cross-crate effects still flow only through events.
pub fn api_router(state: AppState) -> Router {
    let api = Router::new()
        // platform
        .merge(dsoc_auth::routes(state.clone()))
        .merge(dsoc_notify::routes(state.clone()))
        .merge(dsoc_events::routes(state.clone()))
        .merge(dsoc_consensus::routes(state.clone()))
        .merge(dsoc_moderation::routes(state.clone()))
        .merge(dsoc_admin::routes(state.clone()))
        // spaces
        .merge(dsoc_processes::routes(state.clone()))
        .merge(dsoc_assemblies::routes(state.clone()))
        .merge(dsoc_initiatives::routes(state.clone()))
        .merge(dsoc_consultations::routes(state.clone()))
        .merge(dsoc_mandates::routes(state.clone()))
        // components
        .merge(dsoc_proposals::routes(state.clone()))
        .merge(dsoc_votes::routes(state.clone()))
        .merge(dsoc_comments::routes(state.clone()))
        .merge(dsoc_debates::routes(state.clone()))
        .merge(dsoc_meetings::routes(state.clone()))
        .merge(dsoc_budgets::routes(state.clone()))
        .merge(dsoc_surveys::routes(state.clone()))
        .merge(dsoc_accountability::routes(state.clone()))
        .merge(dsoc_consequence::routes(state.clone()))
        .merge(dsoc_scorecard::routes(state.clone()));

    // Serve the static DemocraciaBR front-end (Astro SSG, ADR-0009) at the same origin as the API
    // (no CORS). WEB_ROOT defaults to /srv/web (baked into the image); a missing dir just 404s.
    let web_root = std::env::var("WEB_ROOT").unwrap_or_else(|_| "/srv/web".to_string());
    let static_site = ServeDir::new(web_root).append_index_html_on_directories(true);

    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .nest("/api/v1", api)
        .fallback_service(static_site)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_router_builds() {
        let _app: Router<()> = Router::new().route("/health", get(health));
    }
}
