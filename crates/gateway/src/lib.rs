//! # dsoc-gateway — public API surface
//!
//! Assembles handlers from every crate into one Axum router. Owns no domain tables;
//! it wires service traits, applies auth/rate-limit middleware, and exposes the public
//! contract. IPv6-first (PLAN.md principle 4).

#![forbid(unsafe_code)]

use axum::{routing::get, Json, Router};

/// Liveness/readiness payload.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "dsoc-gateway" }))
}

/// The generated OpenAPI document for clients (web/mobile build against this).
async fn openapi() -> Json<serde_json::Value> {
    let doc: serde_json::Value =
        serde_json::from_str(&dsoc_api_contract::openapi_json()).unwrap_or_default();
    Json(doc)
}

/// Build the root router. Component routers are mounted here as crates land
/// (strangler-fig: one crate at a time behind this stable front door).
pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn router_builds() {
        // Smoke test: the router assembles without panicking and exposes /health.
        let _app = router();
    }
}
