//! Gateway entrypoint. Binds IPv6-first (PLAN.md principle 4): defaults to `[::]:8080`.
//! Phase-0 gate: an empty gateway boots over IPv6 (Zitadel auth wiring lands in Phase 1).

use std::net::{Ipv6Addr, SocketAddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    // IPv6-first: bind the unspecified IPv6 address. IPv4 is an explicit fallback only.
    let port: u16 = std::env::var("GATEWAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "dsoc-gateway listening (IPv6-first)");
    axum::serve(listener, dsoc_gateway::router()).await?;
    Ok(())
}
