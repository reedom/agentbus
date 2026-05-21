//! Global SSE event stream with replay-then-live semantics.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use futures::stream::{self, StreamExt};
use agentbus_core::envelope::{Envelope, Kind};
use agentbus_core::ids::{new_envelope_id, now_utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct EventsQuery {
    pub since: Option<String>,
    pub instance: Option<String>,
    pub kind: Option<String>,
}

pub async fn events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> impl IntoResponse {
    let since = match q.since.as_deref() {
        Some(s) => match agentbus_core::ids::parse_rfc3339(s) {
            Ok(t) => Some(t),
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid since=").into_response(),
        },
        None => None,
    };
    let instance_filter = q.instance.clone();
    let kind_filter = q.kind.clone();

    // Subscribe BEFORE snapshotting the log offset. Otherwise an event appended
    // and broadcast between snapshot_offset() and subscribe() would be lost:
    // it would be absent from the replay batch (appended after our snapshot)
    // AND missed by the live channel (broadcast before our subscription).
    // Ordering subscribe-first ensures any post-snapshot event is queued on
    // the live channel; we then dedup against the replay batch by envelope id
    // since the same event can be both replayed and delivered live (broadcast
    // after subscribe but appended before our snapshot).
    let mut live = state.broadcast_tx.subscribe();
    let snapshot = state.log.snapshot_offset().await.unwrap_or(0);
    let log = state.log.clone();
    let replayed = log
        .replay_since(since, snapshot)
        .await
        .unwrap_or_default();

    let replayed_ids: HashSet<String> = replayed.iter().map(|e| e.id.clone()).collect();

    let inst1 = instance_filter.clone();
    let kind1 = kind_filter.clone();
    let replay_stream = stream::iter(
        replayed
            .into_iter()
            .filter(move |e| matches(e, inst1.as_deref(), kind1.as_deref())),
    );

    let inst2 = instance_filter;
    let kind2 = kind_filter;
    let live_stream = async_stream::stream! {
        loop {
            match live.recv().await {
                Ok(e) => {
                    if replayed_ids.contains(&e.id) { continue; }
                    if matches(&e, inst2.as_deref(), kind2.as_deref()) { yield e; }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Emit a synthetic slow_subscriber event once and continue.
                    let warn = Envelope {
                        id: new_envelope_id(),
                        kind: Kind::Event,
                        from: "system".into(),
                        to: None,
                        request_id: None,
                        timeout_ms: None,
                        ts: now_utc(),
                        payload: serde_json::json!({"type": "slow_subscriber"}),
                    };
                    yield warn;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let combined = replay_stream.chain(live_stream).map(|e| {
        let event = Event::default().data(serde_json::to_string(&e).unwrap());
        Ok::<_, Infallible>(event)
    });

    Sse::new(combined)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn matches(e: &Envelope, instance: Option<&str>, kind: Option<&str>) -> bool {
    if let Some(i) = instance {
        let hit = e.to.as_deref() == Some(i) || e.from == i;
        if !hit {
            return false;
        }
    }
    if let Some(k) = kind {
        let want = match k {
            "message" => Kind::Message,
            "ask" => Kind::Ask,
            "reply" => Kind::Reply,
            "event" => Kind::Event,
            _ => return true,
        };
        if e.kind != want {
            return false;
        }
    }
    true
}

#[derive(Deserialize)]
pub struct PublishReq {
    pub from: Option<String>,
    pub payload: serde_json::Value,
    pub kind: Option<String>,
}

pub async fn publish(
    State(state): State<AppState>,
    Json(req): Json<PublishReq>,
) -> impl IntoResponse {
    let env = Envelope {
        id: new_envelope_id(),
        kind: Kind::Event,
        from: req.from.unwrap_or_else(|| "ext:anonymous".into()),
        to: None,
        request_id: None,
        timeout_ms: None,
        ts: now_utc(),
        payload: serde_json::json!({
            "type": req.kind.unwrap_or_else(|| "event".into()),
            "data": req.payload,
        }),
    };
    let _ = state.log.append(&env).await;
    let _ = state.broadcast_tx.send(env.clone());
    (StatusCode::ACCEPTED, Json(serde_json::json!({"id": env.id}))).into_response()
}
