use serde_json::{Value, json};
use axum::{routing::get, Json, Router};

use crate::appstate::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .with_state(state)
}

pub async fn root_handler() -> Json<Value> {
    Json(json!({"Message": "Convena Rust Tokio API is running!"}))
}
