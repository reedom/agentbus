use clap::Parser;
use mcp_bus_core::eventlog::EventLog;
use mcp_busd::{config, http, shutdown, state};
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = config::Config::parse();
    cfg.validate()?;

    let log = EventLog::open(cfg.resolved_log_path(), cfg.max_payload).await?;
    let app = state::AppState::new(cfg.clone(), log);
    let router = http::build_router(app);

    let addr = SocketAddr::new(cfg.bind, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "mcp-busd listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown::wait_for_shutdown())
        .await?;
    tracing::info!("mcp-busd exiting");
    Ok(())
}
