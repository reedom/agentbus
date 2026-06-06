//! REST handler for blocking `ask` requests.

use agentbus_core::envelope::{Envelope, Kind};
use agentbus_core::ids::{new_envelope_id, now_utc};
use agentbus_core::router::RouteError;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct AskQuery {
    pub timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
pub struct AskReq {
    pub from: Option<String>,
    pub payload: serde_json::Value,
}

pub async fn ask(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<AskQuery>,
    Json(req): Json<AskReq>,
) -> impl IntoResponse {
    // Clamp caller-supplied timeout to [1_000ms, max_timeout_ms]; default when absent.
    let raw = q.timeout_ms.unwrap_or(state.cfg.default_timeout_ms);
    let clamped = raw.min(state.cfg.max_timeout_ms).max(1_000);
    let env = Envelope {
        id: new_envelope_id(),
        kind: Kind::Ask,
        from: req.from.unwrap_or_else(|| "ext:anonymous".into()),
        to: Some(id.clone()),
        request_id: None,
        timeout_ms: Some(clamped),
        ts: now_utc(),
        payload: req.payload,
    };
    let req_id = env.id.clone();
    let _ = state.log.append(&env).await;
    let _ = state.broadcast_tx.send(env.clone());

    match state.router.ask(env, Duration::from_millis(clamped)).await {
        Ok(reply) => {
            let _ = state.log.append(&reply).await;
            let _ = state.broadcast_tx.send(reply.clone());
            (
                StatusCode::OK,
                Json(json!({"request_id": req_id, "payload": reply.payload})),
            )
                .into_response()
        }
        Err(RouteError::AskTimeout { ms }) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({"error": "timeout", "request_id": req_id, "timeout_ms": ms})),
        )
            .into_response(),
        Err(RouteError::UnknownInstance(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown_instance", "request_id": req_id})),
        )
            .into_response(),
        Err(RouteError::InstanceDisconnected(_)) => (
            StatusCode::GONE,
            Json(json!({"error": "instance_disconnected", "request_id": req_id})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
