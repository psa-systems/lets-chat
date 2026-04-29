use axum::{routing::get, Router};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home_stub))
        .nest_service("/assets", ServeDir::new("server/assets"))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn home_stub() -> &'static str {
    "lets-chat (rewrite in progress)"
}
