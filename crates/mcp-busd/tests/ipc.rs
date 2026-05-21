mod common;
use common::*;
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn rpc(stream: &mut UnixStream, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
    let req = json!({"id": id, "method": method, "params": params});
    let mut buf = serde_json::to_vec(&req).unwrap();
    buf.push(b'\n');
    stream.write_all(&buf).await.unwrap();
    let (rd, _) = stream.split();
    let mut reader = BufReader::new(rd);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn ipc_register_then_send() {
    let app = spawn_with_ipc().await;
    let mut stream = UnixStream::connect(&app.ipc_path).await.unwrap();
    let resp = rpc(&mut stream, 1, "register", json!({"instance_id": "bob"})).await;
    assert_eq!(resp["result"]["ok"], true);

    // Send a message to bob from another connection.
    let mut sender = UnixStream::connect(&app.ipc_path).await.unwrap();
    let resp = rpc(
        &mut sender,
        2,
        "send",
        json!({"from": "alice", "to": "bob", "payload": {"hi": 1}}),
    )
    .await;
    assert!(resp["result"]["id"].is_string());

    // bob awaits.
    let resp = rpc(
        &mut stream,
        3,
        "await_message",
        json!({"instance_id": "bob", "timeout_ms": 1000}),
    )
    .await;
    assert_eq!(resp["result"]["envelope"]["payload"]["hi"], 1);
}

#[tokio::test]
async fn ipc_conn_drop_unregisters() {
    let app = spawn_with_ipc().await;
    {
        let mut stream = UnixStream::connect(&app.ipc_path).await.unwrap();
        rpc(&mut stream, 1, "register", json!({"instance_id": "ghost"})).await;
        // drop stream -> triggers conn cleanup
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(app.state.registry.lookup("ghost").await.is_none());
}
