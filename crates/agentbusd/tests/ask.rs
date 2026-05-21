mod common;
use common::*;
use std::time::Duration;

#[tokio::test]
async fn ask_resolves_via_reply() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    // Register bob and start a worker that drains its mailbox and posts a reply.
    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "bob"}))
        .send()
        .await
        .unwrap();

    let bob = app.state.registry.lookup("bob").await.unwrap();
    let addr = app.addr;
    let client2 = client.clone();
    tokio::spawn(async move {
        let req = bob.mailbox.pop().await.unwrap();
        let request_id = req.id.clone();
        client2
            .post(format!("http://{}/v1/instances/bob/replies", addr))
            .json(&serde_json::json!({
                "from": "bob",
                "request_id": request_id,
                "payload": {"pong": true}
            }))
            .send()
            .await
            .unwrap();
    });

    let resp = client
        .post(format!(
            "http://{}/v1/instances/bob/ask?timeout_ms=2000",
            app.addr
        ))
        .json(&serde_json::json!({"from": "ext:test", "payload": {"ping": true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["payload"]["pong"], true);
}

#[tokio::test]
async fn ask_times_out() {
    let app = spawn().await;
    let client = reqwest::Client::new();
    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "bob"}))
        .send()
        .await
        .unwrap();

    let resp = tokio::time::timeout(
        Duration::from_secs(3),
        client
            .post(format!(
                "http://{}/v1/instances/bob/ask?timeout_ms=1000",
                app.addr
            ))
            .json(&serde_json::json!({"payload": {}}))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resp.status(), 504);
}
