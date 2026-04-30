use axum::extract::{Path, State};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::room::{MessageView, RoomPage};
use crate::views::{html, Html};

pub async fn get_room(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Access check: public rooms are visible to everyone; private rooms and
    // DMs require membership. Admins additionally see all non-DM rooms.
    let is_admin = user.role == "admin";
    let can_view = match room.room_type.as_str() {
        "public" => true,
        "dm" => db::chat::is_room_member(&state.chat, room_id, &user.id).await?,
        _ => is_admin || db::chat::is_room_member(&state.chat, room_id, &user.id).await?,
    };
    if !can_view {
        return Err(AppError::Forbidden);
    }

    // Load messages, then resolve each author's username from the auth DB.
    // Cache lookups by user_id to avoid duplicate queries for the same author.
    let raw_messages = db::chat::list_messages(&state.chat, room_id).await?;
    let mut username_cache: HashMap<String, String> = HashMap::new();
    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    for m in raw_messages {
        let username = if let Some(name) = username_cache.get(&m.user_id) {
            name.clone()
        } else {
            let resolved = db::auth::find_user_by_id(&state.auth, &m.user_id)
                .await?
                .map(|r| r.username)
                .unwrap_or_else(|| "(unknown)".to_string());
            username_cache.insert(m.user_id.clone(), resolved.clone());
            resolved
        };
        messages.push(MessageView {
            id: m.id,
            username,
            created_at: m.created_at,
            edited_at: m.edited_at,
            body: m.body,
            // Reactions are populated in Task 11.
            reactions: Vec::new(),
        });
    }

    // Sidebar data (mirrors the home route).
    let rooms = db::chat::list_rooms(&state.chat, &user.id, is_admin).await?;
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peers: Vec<User> = Vec::with_capacity(dm_rooms.len());
    for (_room, peer_id) in &dm_rooms {
        if let Some(record) = db::auth::find_user_by_id(&state.auth, peer_id).await? {
            dm_peers.push(record.into());
        }
    }

    let page = RoomPage {
        user: &user,
        room: &room,
        rooms: &rooms,
        dm_peers: &dm_peers,
        messages: &messages,
        asset_version: state.asset_version,
    };
    html(&page)
}
