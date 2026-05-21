mod common;
use common::*;

#[tokio::test]
async fn message_is_mirrored_to_inbox_file() {
    let app = spawn().await;
    let client = reqwest::Client::new();
    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "bob"}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("http://{}/v1/instances/bob/messages", app.addr))
        .json(&serde_json::json!({"payload": {"k": "v"}}))
        .send()
        .await
        .unwrap();

    let path = app.state.cfg.resolved_inbox_dir().join("bob.jsonl");
    // Allow a moment for the async write.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let content = tokio::fs::read_to_string(path).await.unwrap();
    assert!(content.contains("\"k\":\"v\""));
}
