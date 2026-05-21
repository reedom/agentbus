use std::net::SocketAddr;

#[tokio::test]
async fn health_returns_ok() {
    let app = agentbusd_test_helper().await;
    let resp = reqwest::get(format!("http://{}/v1/health", app))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

async fn agentbusd_test_helper() -> SocketAddr {
    use agentbus_core::eventlog::EventLog;
    use agentbusd::{config::Config, http, state::AppState};
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
    let app = AppState::new_async(cfg.clone(), log).await.unwrap();
    let router = http::build_router(app);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Keep the temp dir alive for the duration of the server.
    tokio::spawn(async move {
        let _tmp = tmp;
        axum::serve(listener, router).await.unwrap();
    });
    addr
}
