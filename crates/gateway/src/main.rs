//! Gateway entrypoint. Binds IPv6-first (PLAN.md principle 4): defaults to `[::]:8080`. Constructs
//! the shared [`AppState`] from the environment (DATABASE_URL + sovereign auth config) and serves
//! the composed API surface.

use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;

use dsoc_app::AppState;
use dsoc_core::clock::SystemClock;
use dsoc_events::PgEventBus;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (IPv6-first, e.g. postgres://dsoc@[::1]/...)")?;
    let db = dsoc_db::connect(&database_url, 16).await?;

    // Localização ativa da instalação (ADR-0015): Pindorama → l10n_br. A instalação declara o país
    // via INSTALL_L10N (ISO-3166-1 alfa-2); default = BR. Resolvida no boot só para log/validação —
    // as rotas (CPF/título/municípios) já consomem dsoc-l10n-br atrás dos traits do core.
    let country = std::env::var("INSTALL_L10N").unwrap_or_else(|_| "BR".to_owned());
    match dsoc_l10n_br::resolve(&country) {
        Some(_) => tracing::info!(country = %country, "l10n ativa: l10n_br (CPF + Título + IBGE)"),
        None => tracing::warn!(
            country = %country,
            "INSTALL_L10N sem módulo l10n_<cc> — só l10n_br está implementado; usando BR"
        ),
    }

    let clock = Arc::new(SystemClock);
    let bus = Arc::new(PgEventBus::new(db.clone()));
    let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
    // Storage is optional: when STORAGE_* env is unset the upload endpoints return 503 and
    // avatars stay blank — useful for dev/test loops that don't need MinIO running. Built
    // async because the AWS SDK loads its default sleep impl + HTTP connector through
    // `aws_config::defaults`.
    let storage = dsoc_storage::S3Storage::from_env()
        .await
        .map(|s| s.into_dyn());
    if storage.is_none() {
        tracing::warn!("STORAGE_* env unset; blob uploads disabled (avatars/covers stay blank)");
    }
    let state = AppState {
        db,
        bus,
        authz,
        clock,
        storage,
    };

    let port: u16 = std::env::var("GATEWAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    // Start the background event worker (the consequence loop's runtime: dispatch + SLA sweep).
    // It shares the same pool/clock and runs for the process lifetime; WORKER_ENABLED=false disables.
    dsoc_gateway::worker::spawn(state.clone());

    let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dsoc-gateway listening (IPv6-first); 21 crates mounted under /api/v1");
    axum::serve(listener, dsoc_gateway::api_router(state)).await?;
    Ok(())
}
