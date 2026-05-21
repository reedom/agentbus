mod common;
use common::*;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::time::Duration;

#[tokio::test]
async fn replay_then_live() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    // Publish 2 events before subscribing.
    for i in 0..2 {
        client
            .post(format!("http://{}/v1/events", app.addr))
            .json(&serde_json::json!({"from": "ext:test", "payload": {"n": i}, "kind": "demo"}))
            .send()
            .await
            .unwrap();
    }

    // Subscribe.
    let resp = client
        .get(format!("http://{}/v1/events", app.addr))
        .send()
        .await
        .unwrap();
    let mut stream = resp.bytes_stream().eventsource();

    // First, read the 2 replayed events.
    let mut got = 0;
    while got < 2 {
        let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&next.data).unwrap();
        assert_eq!(v["payload"]["data"]["n"], got);
        got += 1;
    }

    // Now publish 1 more and read it live.
    client
        .post(format!("http://{}/v1/events", app.addr))
        .json(&serde_json::json!({"from": "ext:test", "payload": {"n": 99}, "kind": "demo"}))
        .send()
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&next.data).unwrap();
    assert_eq!(v["payload"]["data"]["n"], 99);
}

// Regression test for the snapshot/subscribe race window.
//
// Before the fix, the SSE handler called snapshot_offset() BEFORE subscribe().
// Any event appended+broadcast in between was lost: not in replay (appended
// after snapshot) and not in live (subscription started after broadcast).
//
// This test races a publish against an in-flight subscribe. With dedup keyed
// by envelope id and subscribe-first ordering, the event MUST appear exactly
// once: never zero, never twice.
#[tokio::test]
async fn race_window_subscribe_then_publish_yields_event_exactly_once() {
    let app = spawn().await;
    let client = reqwest::Client::new();

    // Launch many trials to actually hit the race window reliably.
    for trial in 0..20 {
        // Spawn a concurrent publish slightly after subscribe begins. The exact
        // interleaving is non-deterministic; over many trials we should land
        // inside the previously-buggy window at least once.
        let publisher_client = client.clone();
        let addr = app.addr;
        let publish_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_micros(50)).await;
            publisher_client
                .post(format!("http://{}/v1/events", addr))
                .json(&serde_json::json!({
                    "from": "ext:race",
                    "payload": {"trial": trial},
                    "kind": "race",
                }))
                .send()
                .await
                .unwrap();
        });

        let resp = client
            .get(format!("http://{}/v1/events", app.addr))
            .send()
            .await
            .unwrap();
        publish_task.await.unwrap();

        let mut stream = resp.bytes_stream().eventsource();

        // Drain events for a short window and count how many match this trial.
        let mut count_for_trial = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(Ok(ev))) => {
                    let v: serde_json::Value = serde_json::from_str(&ev.data).unwrap();
                    if v["from"] == "ext:race" && v["payload"]["data"]["trial"] == trial {
                        count_for_trial += 1;
                    }
                }
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break,
            }
        }

        assert_eq!(
            count_for_trial, 1,
            "trial {trial}: expected exactly one delivery, got {count_for_trial}",
        );
    }
}
