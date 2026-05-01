use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use std::collections::HashMap;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::inject_user;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::layout::{SidebarPeer, SidebarRoom};

mod admin;
mod auth;
mod dm;
mod home;
mod reactions;
mod room;
mod search;
mod ws;

/// Build the sidebar's room and DM-peer view-models for a given user, with
/// per-target unread counts attached. The unread counts come from
/// `dm_read_state`, which is room-keyed and used as a generic per-user
/// watermark for both DM and non-DM rooms.
pub(crate) async fn load_sidebar(
    state: &AppState,
    user: &User,
) -> Result<(Vec<SidebarRoom>, Vec<SidebarPeer>), AppError> {
    let is_admin = user.role == "admin";

    // Rooms section: visible non-DM rooms, in the same order as before.
    let rooms = db::chat::list_rooms(&state.chat, &user.id, is_admin).await?;
    let room_unreads: HashMap<i64, i64> =
        db::chat::list_room_unread_counts(&state.chat, &user.id, is_admin)
            .await?
            .into_iter()
            .collect();
    let sidebar_rooms: Vec<SidebarRoom> = rooms
        .into_iter()
        .map(|r| SidebarRoom {
            unread: *room_unreads.get(&r.id).unwrap_or(&0),
            id: r.id,
            name: r.name,
        })
        .collect();

    // DM peers: list the user's DM rooms then resolve each peer to a public
    // User from the auth DB. The unread count is keyed by DM room_id; we
    // attach it to the peer record (sidebar key for DMs is the peer.id).
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let dm_unreads_by_room: HashMap<i64, i64> =
        db::chat::list_dm_unread_counts(&state.chat, &user.id)
            .await?
            .into_iter()
            .collect();
    let mut sidebar_peers: Vec<SidebarPeer> = Vec::with_capacity(dm_rooms.len());
    for (room, peer_id) in &dm_rooms {
        if let Some(record) = db::auth::find_user_by_id(&state.auth, peer_id).await? {
            sidebar_peers.push(SidebarPeer {
                id: record.id.clone(),
                username: record.username.clone(),
                unread: *dm_unreads_by_room.get(&room.id).unwrap_or(&0),
            });
        }
    }

    Ok((sidebar_rooms, sidebar_peers))
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .route("/login", get(auth::get_login).post(auth::post_login))
        .route("/register", get(auth::get_register).post(auth::post_register))
        .route("/logout", get(auth::get_logout))
        .route("/room/{room_id}", get(room::get_room))
        .route("/room/{room_id}/messages", post(room::post_message))
        .route("/dm/{peer_id}", get(dm::get_dm))
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
        .route("/search", get(search::get_search))
        .route("/ws", get(ws::ws_handler))
        .merge(admin::router())
        .nest_service("/assets", ServeDir::new("server/assets"))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
