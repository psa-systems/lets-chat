use axum::{routing::get, Router};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod home;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .nest_service("/assets", ServeDir::new("server/assets"))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}
