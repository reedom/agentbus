//! REST handlers for sending messages and replies.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use mcp_bus_core::envelope::{Envelope, Kind};
use mcp_bus_core::ids::{new_envelope_id, now_utc};
use mcp_bus_core::router::RouteError;
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct MessageReq {
    pub from: Option<String>,
    pub payload: serde_json::Value,
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<MessageReq>,
) -> impl IntoResponse {
    let env = Envelope {
        id: new_envelope_id(),
        kind: Kind::Message,
        from: req.from.unwrap_or_else(|| "ext:anonymous".into()),
        to: Some(id),
        request_id: None,
        timeout_ms: None,
        ts: now_utc(),
        payload: req.payload,
    };
    match state.router.send(env.clone()).await {
        Ok(id) => {
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env);
            (StatusCode::ACCEPTED, Json(json!({ "id": id }))).into_response()
        }
        Err(RouteError::UnknownInstance(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown_instance"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ReplyReq {
    pub from: Option<String>,
    pub request_id: String,
    pub payload: serde_json::Value,
}

pub async fn send_reply(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ReplyReq>,
) -> impl IntoResponse {
    // The router resolves the recipient by `request_id` alone; `to` must be
    // present to satisfy envelope validation but is otherwise unused in v1.
    let env = Envelope {
        id: new_envelope_id(),
        kind: Kind::Reply,
        from: req.from.unwrap_or(id),
        to: Some("_resolved_via_request_id_".into()),
        request_id: Some(req.request_id.clone()),
        timeout_ms: None,
        ts: now_utc(),
        payload: req.payload,
    };
    match state.router.reply(env.clone()).await {
        Ok(()) => {
            let _ = state.log.append(&env).await;
            let _ = state.broadcast_tx.send(env);
            (StatusCode::ACCEPTED, Json(json!({"ok": true}))).into_response()
        }
        Err(RouteError::UnknownRequestId(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown_request_id"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
