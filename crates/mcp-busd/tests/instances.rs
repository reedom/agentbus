mod common;
use common::*;

#[tokio::test]
async fn register_then_list_then_unregister() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    // register
    let resp = client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["owner_token"].as_str().unwrap().to_string();

    // list shows alice
    let resp = client
        .get(format!("http://{}/v1/instances", app.addr))
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = resp.json().await.unwrap();
    assert!(list["instances"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == "alice"));

    // collision
    let resp = client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // unregister
    let resp = client
        .delete(format!("http://{}/v1/instances/alice", app.addr))
        .header("x-mcp-bus-owner", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // list empty
    let list: serde_json::Value = client
        .get(format!("http://{}/v1/instances", app.addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list["instances"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn unregister_with_wrong_token_rejected() {
    let app = spawn().await;
    let client = reqwest::Client::new();
    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "x"}))
        .send()
        .await
        .unwrap();
    let resp = client
        .delete(format!("http://{}/v1/instances/x", app.addr))
        .header("x-mcp-bus-owner", "999999")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}
