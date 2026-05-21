//! REST handlers for instance lifecycle: register, list, unregister.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterReq {
    pub instance_id: String,
    #[serde(default = "default_mailbox_size")]
    pub mailbox_size: usize,
}

fn default_mailbox_size() -> usize {
    256
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterReq>,
) -> impl IntoResponse {
    let owner = state.registry.issue_owner_token();
    match state
        .registry
        .register(&req.instance_id, owner, req.mailbox_size)
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "instance_id": req.instance_id,
                "owner_token": owner.to_string(),
            })),
        )
            .into_response(),
        Err(agentbus_core::registry::RegisterError::Collision(id)) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "instance_id_taken", "instance_id": id})),
        )
            .into_response(),
        Err(agentbus_core::registry::RegisterError::Invalid) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_instance_id"})),
        )
            .into_response(),
    }
}

pub async fn unregister(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token: u64 = match headers
        .get("x-agentbus-owner")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
    {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing_owner_token"})),
            )
                .into_response();
        }
    };
    if state.registry.unregister(&id, token).await {
        // Cancel any pending ask() awaiters whose target was this instance.
        state.router.cancel_pending_for(&id).await;
        (StatusCode::NO_CONTENT, ()).into_response()
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "owner_mismatch_or_unknown"})),
        )
            .into_response()
    }
}

pub async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let ids = state.registry.list_ids().await;
    Json(json!({
        "instances": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
    }))
}
