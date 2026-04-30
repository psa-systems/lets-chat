use axum::{middleware, routing::get, Router};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::inject_user;
use crate::state::AppState;

mod home;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .nest_service("/assets", ServeDir::new("server/assets"))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
