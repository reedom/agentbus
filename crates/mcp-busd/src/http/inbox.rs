//! Per-instance SSE inbox stream.
//!
//! Drains the per-instance mailbox and emits each envelope as `data: <json>\n\n`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures::stream;
use std::convert::Infallible;
use std::time::Duration;

use crate::state::AppState;

pub async fn inbox(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let rec = match state.registry.lookup(&id).await {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "unknown_instance").into_response(),
    };
    let mailbox = rec.mailbox.clone();
    let s = stream::unfold(mailbox, |mb| async move {
        match mb.pop().await {
            Ok(env) => {
                let event = Event::default().data(serde_json::to_string(&env).unwrap());
                Some((Ok::<_, Infallible>(event), mb))
            }
            Err(_closed) => None,
        }
    });
    Sse::new(s)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
