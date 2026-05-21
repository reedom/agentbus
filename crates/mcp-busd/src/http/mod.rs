//! HTTP router assembly.

use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;

use crate::state::AppState;

pub mod ask;
pub mod instances;
pub mod messages;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route(
            "/v1/instances",
            post(instances::register).get(instances::list),
        )
        .route("/v1/instances/:id", delete(instances::unregister))
        .route(
            "/v1/instances/:id/messages",
            post(messages::send_message),
        )
        .route("/v1/instances/:id/replies", post(messages::send_reply))
        .route("/v1/instances/:id/ask", post(ask::ask))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
