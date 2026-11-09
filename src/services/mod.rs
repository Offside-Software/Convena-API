use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::appstate::AppState;

pub mod auth;
pub mod events;
pub mod finance;
pub mod organizations;
pub mod rbac;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .nest("/api/auth", auth::auth_routes())
        .nest("/auth", auth::auth_routes())
        .nest("/api/events", events::event_routes())
        .nest("/events", events::event_routes())
        .nest("/api/finances", finance::finance_routes())
        .nest("/finances", finance::finance_routes())
        .nest("/api/organizations", organizations::organization_routes())
        .nest("/organizations", organizations::organization_routes())
        .with_state(state)
}

pub async fn root_handler() -> Json<Value> {
    Json(json!({"Message": "Convena Rust Tokio API is running!"}))
}
