//! # dsoc-gateway — public API surface
//!
//! Assembles `routes(AppState)` from every crate into one Axum router (the strangler-fig front
//! door, PLAN.md section 8). Owns no domain tables; it composes the crate routers, serves the
//! OpenAPI contract, and exposes health. IPv6-first (PLAN.md principle 4).

#![forbid(unsafe_code)]

pub mod federation;
pub mod worker;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{routing::get, Json, Router};
use dsoc_app::AppState;
use uuid::Uuid;
use tower_http::services::ServeDir;

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "dsoc-gateway" }))
}

async fn openapi() -> Json<serde_json::Value> {
    let doc: serde_json::Value =
        serde_json::from_str(&dsoc_api_contract::openapi_json()).unwrap_or_default();
    Json(doc)
}

/// Auth middleware: resolve the `dsoc_session` cookie to the caller's identity and inject the
/// standard headers downstream handlers expect (`x-dsoc-citizen-id`/`x-dsoc-org-id`, plus
/// `x-citizen-id` for admin). Anonymous requests pass through with no headers added.
async fn inject_identity(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let session_id = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, "dsoc_session"))
        .and_then(|s| Uuid::parse_str(s).ok());
    if let Some(sid) = session_id {
        if let Ok(Some((citizen, org))) = dsoc_auth::session_identity(&state.db, sid).await {
            let headers = req.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&citizen.to_string()) {
                headers.insert("x-dsoc-citizen-id", v.clone());
                headers.insert("x-citizen-id", v);
            }
            if let Ok(v) = HeaderValue::from_str(&org.to_string()) {
                headers.insert("x-dsoc-org-id", v);
            }
        }
    }
    next.run(req).await
}

/// Extract a cookie value by name from a `Cookie` header.
fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|kv| {
        let kv = kv.trim();
        kv.strip_prefix(name).and_then(|rest| rest.strip_prefix('='))
    })
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
        .merge(dsoc_scorecard::routes(state.clone()))
        .layer(middleware::from_fn_with_state(state.clone(), inject_identity));

    // Serve the static DemocraciaBR front-end (Astro SSG, ADR-0009) at the same origin as the API
    // (no CORS). WEB_ROOT defaults to /srv/web (baked into the image); a missing dir just 404s.
    let web_root = std::env::var("WEB_ROOT").unwrap_or_else(|_| "/srv/web".to_string());
    let static_site = ServeDir::new(web_root).append_index_html_on_directories(true);

    // Federation surface lives at the root (NOT under /api/v1), so paths look like any other
    // ActivityPub instance: /.well-known/webfinger, /actors/<handle>. The federation module is
    // DB-backed (resolves @handle to a public citizen) — see ADR-0010 W2.
    let federation_routes = federation::routes(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .nest("/api/v1", api)
        .merge(federation_routes)
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
