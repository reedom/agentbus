//! Shared helpers for integration tests.

use std::net::SocketAddr;

use mcp_bus_core::eventlog::EventLog;
use mcp_busd::{config::Config, http, state::AppState};

pub struct App {
    pub addr: SocketAddr,
    pub _tmp: tempfile::TempDir,
}

pub async fn spawn() -> App {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        socket: Some(tmp.path().join("s.sock")),
        log_path: Some(tmp.path().join("e.jsonl")),
        max_payload: 65536,
        inbox_dir: Some(tmp.path().join("inbox")),
        default_timeout_ms: 30_000,
        max_timeout_ms: 86_400_000,
    };
    let log = EventLog::open(cfg.resolved_log_path(), cfg.max_payload)
        .await
        .unwrap();
    let state = AppState::new(cfg, log);
    let router = http::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    App { addr, _tmp: tmp }
}
