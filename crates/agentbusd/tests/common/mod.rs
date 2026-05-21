//! Shared helpers for integration tests.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use agentbus_core::eventlog::EventLog;
use agentbusd::{config::Config, http, state::AppState};

pub struct App {
    pub addr: SocketAddr,
    pub state: AppState,
    pub _tmp: tempfile::TempDir,
}

pub struct AppWithIpc {
    pub addr: SocketAddr,
    pub state: AppState,
    pub ipc_path: PathBuf,
    pub _tmp: tempfile::TempDir,
}

pub async fn spawn_with_ipc() -> AppWithIpc {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("s.sock");
    let cfg = Config {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        socket: Some(socket.clone()),
        log_path: Some(tmp.path().join("e.jsonl")),
        max_payload: 65536,
        inbox_dir: Some(tmp.path().join("inbox")),
        default_timeout_ms: 30_000,
        max_timeout_ms: 86_400_000,
    };
    let log = EventLog::open(cfg.resolved_log_path(), cfg.max_payload)
        .await
        .unwrap();
    let state = AppState::new_async(cfg, log).await.unwrap();
    let router = http::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let st2 = state.clone();
    let sp = socket.clone();
    tokio::spawn(async move {
        agentbusd::ipc::serve(st2, sp).await.unwrap();
    });

    // Wait briefly for the listener to bind before tests connect.
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    AppWithIpc {
        addr,
        state,
        ipc_path: socket,
        _tmp: tmp,
    }
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
    let state = AppState::new_async(cfg, log).await.unwrap();
    let router = http::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    App {
        addr,
        state,
        _tmp: tmp,
    }
}
