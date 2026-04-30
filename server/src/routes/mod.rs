use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::inject_user;
use crate::state::AppState;

mod auth;
mod home;
mod reactions;
mod room;
mod ws;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .route("/login", get(auth::get_login).post(auth::post_login))
        .route("/register", get(auth::get_register).post(auth::post_register))
        .route("/logout", get(auth::get_logout))
        .route("/room/{room_id}", get(room::get_room))
        .route("/room/{room_id}/messages", post(room::post_message))
        .route(
            "/messages/{message_id}",
            get(room::get_single_message)
                .patch(room::patch_message)
                .delete(room::delete_message),
        )
        .route("/messages/{message_id}/edit", get(room::get_edit_form))
        .route(
            "/messages/{message_id}/reactions/picker",
            get(reactions::get_picker),
        )
        .route(
            "/messages/{message_id}/reactions/cancel",
            get(reactions::cancel_picker),
        )
        .route(
            "/messages/{message_id}/reactions/{emoji}",
            post(reactions::toggle_reaction),
        )
        .route("/ws", get(ws::ws_handler))
        .nest_service("/assets", ServeDir::new("server/assets"))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
