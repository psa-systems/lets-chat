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
use crate::models::{Room, User};
use crate::state::AppState;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::ws::events::ChatEvent;

mod admin;
mod auth;
mod dm;
mod enclave;
mod home;
mod reactions;
mod room;
mod search;
mod settings;
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

/// Convenience wrapper that returns sidebar lists plus the switcher entries
/// in one call. Most page handlers want all three for layout.html.
pub(crate) async fn load_chrome(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<(Vec<SidebarRoom>, Vec<SidebarPeer>, Vec<SwitcherEntry>), AppError> {
    let (rooms, peers) = load_sidebar(state, user).await?;
    let switcher = load_switcher(state, user, current_enclave).await?;
    Ok((rooms, peers, switcher))
}

/// Build the leftmost switcher column: a Home entry plus one icon per
/// enclave the caller is a member of. The `current_enclave` argument
/// highlights the active icon (None = Home).
pub(crate) async fn load_switcher(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<Vec<SwitcherEntry>, AppError> {
    let is_admin = user.role == "admin";

    let dm_unread: i64 = db::chat::list_dm_unread_counts(&state.chat, &user.id)
        .await?
        .iter()
        .map(|(_, c)| *c)
        .sum();
    let pending_invites = db::enclave::list_invitations_for_user(&state.chat, &user.id)
        .await?
        .len() as i64;

    let mut entries = Vec::new();
    entries.push(SwitcherEntry {
        id: None,
        label: "Home".to_string(),
        initial: "H".to_string(),
        unread: dm_unread,
        pending_invites,
        active: current_enclave.is_none(),
    });

    let enclaves = db::enclave::list_enclaves_for_user(&state.chat, &user.id).await?;
    let _ = is_admin;
    for e in enclaves {
        let initial = e
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        entries.push(SwitcherEntry {
            id: Some(e.id),
            label: e.name,
            initial,
            // Per-enclave unread aggregation is added in Phase 4 alongside
            // EnclaveRoomAdded broadcasts; for now the icon shows no badge.
            unread: 0,
            pending_invites: 0,
            active: current_enclave == Some(e.id),
        });
    }

    Ok(entries)
}

/// Broadcast a `NewMessage` (or other room-wide) event to every user that has
/// the room visible in their sidebar - room members for private/DM rooms, all
/// connected users for public rooms. Each recipient's WebSocket handler then
/// decides how to render based on whether the connection is currently
/// subscribed to the room (open in the foreground) or not (sidebar-only,
/// renders an unread bump). Replaces the narrower `broadcast_to_room` so
/// sidebar badges update live for users who are not actively viewing the
/// room when a new message arrives.
pub(crate) async fn broadcast_room_message(
    state: &AppState,
    room: &Room,
    event: &ChatEvent,
) -> Result<(), AppError> {
    let recipients: Vec<String> = match room.room_type.as_str() {
        "public" => state.hub.list_connected_users(),
        _ => db::chat::list_room_member_ids(&state.chat, room.id).await?,
    };
    for uid in recipients {
        state.hub.broadcast_to_user(&uid, event);
    }
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home::get_home))
        .route("/login", get(auth::get_login).post(auth::post_login))
        .route(
            "/register",
            get(auth::get_register).post(auth::post_register),
        )
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
        .route(
            "/settings",
            get(settings::get_settings).post(settings::post_settings),
        )
        .route("/ws", get(ws::ws_handler))
        .merge(enclave::router())
        .merge(admin::router())
        .nest_service("/assets", ServeDir::new("server/assets"))
        .layer(middleware::from_fn_with_state(state.clone(), inject_user))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
