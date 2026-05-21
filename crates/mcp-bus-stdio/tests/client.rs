use std::sync::Arc;
use std::time::Duration;

use mcp_bus_stdio::uds_client::UdsClient;

#[tokio::test]
async fn client_register_and_send_and_await() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("s.sock");
    let log_path = tmp.path().join("e.jsonl");
    let cfg = mcp_busd::config::Config {
        bind: "127.0.0.1".parse().unwrap(),
        port: 0,
        socket: Some(socket.clone()),
        log_path: Some(log_path.clone()),
        max_payload: 65536,
        inbox_dir: Some(tmp.path().join("inbox")),
        default_timeout_ms: 30_000,
        max_timeout_ms: 86_400_000,
    };
    let log = mcp_bus_core::eventlog::EventLog::open(&log_path, 65536)
        .await
        .unwrap();
    let state = mcp_busd::state::AppState::new_async(cfg, log).await.unwrap();
    let st = state.clone();
    let sp = socket.clone();
    tokio::spawn(async move { mcp_busd::ipc::serve(st, sp).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let bob = Arc::new(UdsClient::new(socket.clone()));
    bob.call("register", serde_json::json!({"instance_id": "bob"}))
        .await
        .unwrap();

    let alice = Arc::new(UdsClient::new(socket));
    alice
        .call(
            "send",
            serde_json::json!({"from": "alice", "to": "bob", "payload": {"hi": 1}}),
        )
        .await
        .unwrap();

    let res = bob
        .call(
            "await_message",
            serde_json::json!({"instance_id": "bob", "timeout_ms": 1000}),
        )
        .await
        .unwrap();
    assert_eq!(res["envelope"]["payload"]["hi"], 1);
}
