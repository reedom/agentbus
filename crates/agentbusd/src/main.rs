use agentbus_core::eventlog::EventLog;
use agentbusd::{config, http, shutdown, state};
use clap::Parser;
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
    let app = state::AppState::new_async(cfg.clone(), log).await?;
    let router = http::build_router(app.clone());

    let ipc_path = cfg.resolved_socket();
    let ipc_state = app.clone();
    tokio::spawn(async move {
        if let Err(e) = agentbusd::ipc::serve(ipc_state, ipc_path).await {
            tracing::error!(error = %e, "ipc serve failed");
        }
    });

    let addr = SocketAddr::new(cfg.bind, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "agentbusd listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown::wait_for_shutdown())
        .await?;
    tracing::info!("agentbusd exiting");
    Ok(())
}
