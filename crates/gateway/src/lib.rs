//! # dsoc-gateway — public API surface
//!
//! Assembles `routes(AppState)` from every crate into one Axum router (the strangler-fig front
//! door, PLAN.md section 8). Owns no domain tables; it composes the crate routers, serves the
//! OpenAPI contract, and exposes health. IPv6-first (PLAN.md principle 4).

#![forbid(unsafe_code)]

pub mod admin_ext;
pub mod amendments;
pub mod discovery;
pub mod elections;
pub mod federation;
pub mod me_settings;
pub mod politicos_ext;
pub mod social_graph;
pub mod federation_feed;
pub mod mastodon_api;
pub mod mastodon_dto;
pub mod mastodon_oauth;
pub mod note_media;
pub mod notifications;
pub mod parlamentar_activity;
pub mod polls;
pub mod reports;
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

/// Auth middleware: resolve the `dsoc_session` cookie OR the
/// `Authorization: Bearer <token>` header (Mastodon Client API) to the
/// caller's identity, then inject the standard downstream headers
/// (`x-dsoc-citizen-id` / `x-dsoc-org-id`, plus `x-citizen-id` for admin).
/// Anonymous requests pass through with no headers added.
async fn inject_identity(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    // First: cookie session (the site's own flow).
    let mut resolved: Option<(Uuid, Uuid)> = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, "dsoc_session"))
        .and_then(|s| Uuid::parse_str(s).ok())
        .and_then(|sid| Some((sid, ())))
        .map(|(sid, ())| sid)
        .and_then(|_sid| None::<(Uuid, Uuid)>); // placeholder — resolved async below
    if let Some(sid) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, "dsoc_session"))
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        if let Ok(Some((citizen, org))) = dsoc_auth::session_identity(&state.db, sid).await {
            resolved = Some((citizen, org));
        }
    }
    // Second: Mastodon-compatible Bearer token. Only checked when cookie
    // didn't resolve (cookies win for the site's own web).
    if resolved.is_none() {
        if let Some((citizen, org)) =
            crate::mastodon_api::resolve_bearer_to_headers(&state, req.headers()).await
        {
            resolved = Some((citizen, org));
        }
    }
    if let Some((citizen, org)) = resolved {
        let headers = req.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&citizen.to_string()) {
            headers.insert("x-dsoc-citizen-id", v.clone());
            headers.insert("x-citizen-id", v);
        }
        if let Ok(v) = HeaderValue::from_str(&org.to_string()) {
            headers.insert("x-dsoc-org-id", v);
        }
    }
    next.run(req).await
}

/// Extract a cookie value by name from a `Cookie` header.
pub(crate) fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
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
        .merge(admin_ext::routes(state.clone()))
        // spaces
        .merge(dsoc_processes::routes(state.clone()))
        .merge(dsoc_assemblies::routes(state.clone()))
        .merge(dsoc_initiatives::routes(state.clone()))
        .merge(dsoc_consultations::routes(state.clone()))
        .merge(dsoc_mandates::routes(state.clone()))
        // Catálogo de partidos + diretórios subnacionais + administradores (Fase 2B,
        // migration 0204). Rotas públicas: `/api/v1/parties`, `/api/v1/parties/{sigla}`.
        .merge(dsoc_mandates::parties_routes(state.clone()))
        // Gateway-owned proxy of each parliamentarian's real public activity from the official
        // open-data APIs (Câmara/Senado). Path: `/api/v1/mandates/{id}/atividade`.
        .merge(parlamentar_activity::routes(state.clone()))
        // Aggregated dashboards: gasto parlamentar + proposals summary.
        .merge(reports::routes(state.clone()))
        // Filtered politicos browser (0.23.0-municipais).
        .merge(politicos_ext::routes(state.clone()))
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
        // Federation client surface: `/federation/lookup`, `/me/follow` — see ADR-0010 W2.4.
        // Goes UNDER the same `/api/v1` prefix so the cookie/identity middleware below covers it.
        .merge(federation::client_routes(state.clone()))
        // Per-citizen settings: authorized OAuth apps + change password.
        .merge(me_settings::routes(state.clone()))
        // Social-graph endpoints (bookmarks, mutes, blocks, filters, lists —
        // migration 0500). Mastodon-parity fase 2A.
        .merge(social_graph::routes(state.clone()))
        // Decidim-parity amendments (migration 0501).
        .merge(amendments::routes(state.clone()))
        // 2026 elections read-only surface (migration 0502).
        .merge(elections::routes(state.clone()))
        // Mastodon Client API compat (Ivory / Elk / Ice Cubes / Tusky /
        // custom scripts). Same prefix; the Mastodon paths don't collide
        // with ours. Auth is bearer OR cookie (see `inject_identity`).
        .merge(mastodon_api::masto_routes(state.clone()))
        .layer(middleware::from_fn_with_state(state.clone(), inject_identity));

    // Serve the static DemocraciaBR front-end (Astro SSG, ADR-0009) at the same origin as the API
    // (no CORS). WEB_ROOT defaults to /srv/web (baked into the image); a missing dir just 404s.
    let web_root = std::env::var("WEB_ROOT").unwrap_or_else(|_| "/srv/web".to_string());
    let static_site = ServeDir::new(web_root).append_index_html_on_directories(true);

    // Federation PUBLIC surface lives at the root (NOT under /api/v1): /.well-known/webfinger,
    // /actors/<handle>, /actors/<handle>/{inbox,outbox,followers,following}. These are read by
    // remote ActivityPub instances and must keep their canonical paths. (The federation CLIENT
    // surface — what the front uses to look up + follow remote actors — is merged INTO `api`
    // above so the cookie/identity middleware covers it.)
    let federation_public = federation::public_routes(state.clone());
    let oauth = mastodon_api::oauth_routes(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .nest("/api/v1", api)
        .merge(federation_public)
        .merge(oauth)
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
