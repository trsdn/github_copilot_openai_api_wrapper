pub mod auth;
pub mod config;
pub mod copilot;
pub mod models;
pub mod routes;

use std::sync::Arc;
use axum::Router;
use tower_http::cors::CorsLayer;

use copilot::CopilotClient;

/// Shared application state.
pub struct AppState {
    pub client: CopilotClient,
}

/// Build the application router (used by main and integration tests).
pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(routes::router())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
