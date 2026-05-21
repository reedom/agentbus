mod common;
use common::*;
use agentbus_core::envelope::Envelope;
use std::time::Duration;

#[tokio::test]
async fn message_delivered_to_registered_instance() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    // Register receiver.
    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "bob"}))
        .send()
        .await
        .unwrap();

    // External sender posts a message.
    let resp = client
        .post(format!("http://{}/v1/instances/bob/messages", app.addr))
        .json(&serde_json::json!({"from": "ext:test", "payload": {"hi": 1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Drain via internal registry (simulates an MCP client awaiting).
    let bob = app.state.registry.lookup("bob").await.unwrap();
    let env: Envelope = tokio::time::timeout(Duration::from_secs(1), bob.mailbox.pop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(env.payload["hi"], 1);
}

#[tokio::test]
async fn message_to_unknown_returns_404() {
    let app = spawn().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/instances/ghost/messages", app.addr))
        .json(&serde_json::json!({"payload": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
