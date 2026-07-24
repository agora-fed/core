//! # dsoc-gateway — public API surface
//!
//! Assembles `routes(AppState)` from every crate into one Axum router (the strangler-fig front
//! door, PLAN.md section 8). Owns no domain tables; it composes the crate routers, serves the
//! OpenAPI contract, and exposes health. IPv6-first (PLAN.md principle 4).

#![forbid(unsafe_code)]
// The gateway's HTTP surface reads dozens of ad-hoc runtime `sqlx::query_as`
// tuple rows (policy: no `.sqlx/` cache for this crate). Naming every 5+-field
// tuple would add ~24 single-use aliases with no call-site reuse, so the
// complexity lint is accepted crate-wide here (domain crates keep it denied).
#![allow(clippy::type_complexity)]

pub mod admin_content;
pub mod admin_ext;
pub mod admin_reports;
pub mod admin_users;
pub mod amendments;
pub mod announcements;
pub mod attestations;
pub mod audience;
pub mod campaign_groups;
pub mod campanha;
pub mod civic_notify;
pub mod contact;
pub mod discovery;
pub mod elections;
pub mod email_templates;
pub mod embed;
pub mod federation;
pub mod federation_feed;
pub mod fediverso_admin;
pub mod govbr_oidc;
pub mod invitations;
pub mod invite_campaign;
pub mod lgpd;
pub mod mastodon_api;
pub mod mastodon_dto;
pub mod mastodon_oauth;
pub mod me_settings;
pub mod note_media;
pub mod notification_receipts;
pub mod notifications;
pub mod og_cards;
pub mod parlamentar_activity;
pub mod politicos_ext;
pub mod polls;
pub mod preferences;
pub mod proposal_delivery;
pub mod public_stats;
pub mod rate_limit;
pub mod reports;
pub mod respond_link;
pub mod signup_gates;
pub mod social_graph;
pub mod threshold_policy;
pub mod titulo_eleitor;
pub mod web_push;
pub mod webhooks;
pub mod worker;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{routing::get, Json, Router};
use dsoc_app::AppState;
use tower_http::services::ServeDir;
use uuid::Uuid;

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
    // SECURITY (2026-07-24): estes headers SÃO o sinal de caller autenticado lá embaixo
    // (CallerId em crates/app/src/caller.rs e require_admin leem eles). Um cliente NUNCA pode
    // fornecê-los — senão qualquer um personifica qualquer cidadão (inclusive admin). Apaga
    // toda cópia vinda do cliente ANTES de resolver a sessão; só um cookie/bearer real volta a
    // setá-los abaixo. Defesa em profundidade: o ingress Caddy também os remove
    // (deploy/caddy/Caddyfile).
    {
        let headers = req.headers_mut();
        headers.remove("x-dsoc-citizen-id");
        headers.remove("x-dsoc-org-id");
        headers.remove("x-citizen-id");
    }
    // First: cookie session (the site's own flow); resolved async below.
    let mut resolved: Option<(Uuid, Uuid)> = None;
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
        kv.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
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
        // Super-admin: editar/ocultar/apagar mandato, proposta, partido (0.40, SOCRATES).
        .merge(admin_content::routes(state.clone()))
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
        // Cidadania política — validação do título de eleitor.
        .merge(titulo_eleitor::routes(state.clone()))
        // Web Push (0.25.0-fediverso): subscribe + GET da chave VAPID pública.
        .merge(web_push::routes(state.clone()))
        // Templates de e-mail editáveis (admin CRUD).
        .merge(email_templates::routes(state.clone()))
        .merge(invite_campaign::routes(state.clone()))
        .merge(audience::routes(state.clone()))
        // GUI completa de usuários (admin CRUD).
        .merge(admin_users::routes(state.clone()))
        .merge(admin_reports::routes(state.clone()))
        .merge(invitations::routes(state.clone()))
        .merge(announcements::routes(state.clone()))
        .merge(preferences::routes(state.clone()))
        .merge(fediverso_admin::routes(state.clone()))
        .merge(signup_gates::routes(state.clone()))
        // Atestado de cidadania por operador verificado (0.28.3).
        .merge(attestations::routes(state.clone()))
        // Prova de notificação — timeline pública dos avisos ao gabinete (0.29).
        .merge(notification_receipts::routes(state.clone()))
        // Reply-to-respond — gabinete responde via link assinado, sem conta (0.30).
        .merge(respond_link::routes(state.clone()))
        // Preview público do gatilho dinâmico (0.30.3) — o form mostra a regra.
        .merge(threshold_policy::routes(state.clone()))
        // Doações/financiamento de campanha — gated por vínculo de mandato (0.31).
        .merge(campanha::routes(state.clone()))
        // Grupos de campanha — canal proativo campanha→eleitor (0.39, Fase 2.3).
        .merge(campaign_groups::routes(state.clone()))
        // Formulário de contato público — nenhum e-mail exposto no site.
        .merge(contact::routes(state.clone()))
        .merge(webhooks::routes(state.clone()))
        // gov.br OIDC status (só o "enabled?"). Start/callback ficam na raiz.
        .merge(govbr_oidc::api_routes(state.clone()))
        // LGPD art. 18 — exportar/excluir dados pessoais.
        .merge(lgpd::routes(state.clone()))
        // Estatísticas públicas — usadas na landing pra reforçar a tese.
        .merge(public_stats::routes(state.clone()))
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
        // Rate-limit de escrita (0.42.0): camada MAIS INTERNA — roda depois do
        // inject_identity, então já enxerga o x-dsoc-citizen-id pra chavear por
        // cidadão (anônimo cai no IP). Só conta métodos mutantes.
        .layer(middleware::from_fn(rate_limit::rate_limit_middleware))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            inject_identity,
        ))
        // Regras de registro (0.28.2): email_domain_block + ip_rule valem
        // de fato em register/login — administradas em /admin/email-domains
        // e /admin/ip-rules.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            signup_gates::gates_middleware,
        ))
        // Threshold dinâmico (0.30.1): o gatilho da proposta é fração do
        // eleitorado TSE do território, com piso/teto — o autor não escolhe.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            threshold_policy::threshold_middleware,
        ));

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
        // Placar embedável pra imprensa (0.30.2) — raiz, URL limpa pra iframe.
        .merge(embed::routes(state.clone()))
        // OG card PNG do placar (0.33.0) — raiz, apontada pelos og:image.
        .merge(og_cards::routes(state.clone()))
        .merge(oauth)
        // gov.br OIDC — start/callback na raiz porque o gov.br exige que
        // `redirect_uri` seja exatamente `<origin>/auth/govbr/callback`.
        .merge(govbr_oidc::root_routes(state.clone()))
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
