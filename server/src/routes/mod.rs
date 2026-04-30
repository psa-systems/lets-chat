use axum::{middleware, routing::get, Router};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::inject_user;
use crate::state::AppState;

mod auth;
mod home;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .route("/login", get(auth::get_login).post(auth::post_login))
        .route("/register", get(auth::get_register).post(auth::post_register))
        .route("/logout", get(auth::get_logout))
        .nest_service("/assets", ServeDir::new("server/assets"))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
