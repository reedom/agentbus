mod config;
mod shutdown;
mod state;

use clap::Parser;
use mcp_bus_core::eventlog::EventLog;
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
    let _app = state::AppState::new(cfg.clone(), log);

    tracing::info!(
        addr = %cfg.bind,
        port = cfg.port,
        "mcp-busd starting (HTTP server attaches in later task)"
    );
    shutdown::wait_for_shutdown().await;
    tracing::info!("mcp-busd exiting");
    Ok(())
}
