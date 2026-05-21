//! HTTP router assembly.

use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;

use crate::state::AppState;

pub mod instances;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route(
            "/v1/instances",
            post(instances::register).get(instances::list),
        )
        .route("/v1/instances/:id", delete(instances::unregister))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
