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

    let clock = Arc::new(SystemClock);
    let bus = Arc::new(PgEventBus::new(db.clone()));
    let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
    let state = AppState {
        db,
        bus,
        authz,
        clock,
    };

    let port: u16 = std::env::var("GATEWAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dsoc-gateway listening (IPv6-first); 21 crates mounted under /api/v1");
    axum::serve(listener, dsoc_gateway::api_router(state)).await?;
    Ok(())
}
