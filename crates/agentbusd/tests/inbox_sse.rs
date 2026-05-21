mod common;
use common::*;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::time::Duration;

#[tokio::test]
async fn inbox_streams_messages() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    client
        .post(format!("http://{}/v1/instances", app.addr))
        .json(&serde_json::json!({"instance_id": "bob"}))
        .send()
        .await
        .unwrap();

    let addr = app.addr;
    let client2 = client.clone();
    let pusher = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        for i in 0..3 {
            client2
                .post(format!("http://{}/v1/instances/bob/messages", addr))
                .json(&serde_json::json!({"payload": {"n": i}}))
                .send()
                .await
                .unwrap();
        }
    });

    let resp = client
        .get(format!("http://{}/v1/instances/bob/inbox", app.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream().eventsource();
    let mut received = Vec::new();
    while received.len() < 3 {
        let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap();
        let ev = next.unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
        received.push(v);
    }
    assert_eq!(received[0]["payload"]["n"], 0);
    assert_eq!(received[2]["payload"]["n"], 2);
    pusher.await.unwrap();
}
