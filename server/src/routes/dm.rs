use axum::extract::{Path, State};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::dm::DmPage;
use crate::views::room::{MessageView, ReactionView};
use crate::views::{html, Html};

/// GET /dm/{peer_id} - render a DM view between the authenticated user and
/// the peer. Resolves to (or creates) a room with `room_type = "dm"` and
/// renders the same composer/messages layout used by regular rooms.
pub async fn get_dm(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(peer_id): Path<String>,
) -> Result<Html, AppError> {
    // Self-DMs are not supported.
    if peer_id == user.id {
        return Err(AppError::BadRequest(
            "cannot open a DM with yourself".into(),
        ));
    }

    // Resolve the peer; 404 if missing.
    let peer_record = db::auth::find_user_by_id(&state.auth, &peer_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let peer: User = peer_record.into();

    // Find or create the underlying DM room. The DM room is named after the
    // peer relative to the creator; the rendered title uses peer.username
    // directly so the displayed name matches whoever the viewer is talking to.
    let room = match db::chat::find_dm_room(&state.chat, &user.id, &peer.id).await? {
        Some(r) => r,
        None => {
            let dm_name = format!("@{}", peer.username);
            db::chat::create_dm_room(&state.chat, &dm_name, &user.id, &peer.id).await?
        }
    };
    let room_id = room.id;

    // Defensive membership check. find_dm_room/create_dm_room should always
    // produce a room the current user is a member of, but verify so a stale
    // row can never bypass access control.
    if !db::chat::is_room_member(&state.chat, room_id, &user.id).await? {
        return Err(AppError::Forbidden);
    }

    // Mirror routes/room.rs: load messages, resolve usernames, attach reactions.
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
        let can_edit = m.user_id == user.id;
        let reactions: Vec<ReactionView> = db::chat::list_reactions(&state.chat, m.id, &user.id)
            .await?
            .into_iter()
            .map(|r| ReactionView {
                emoji: r.emoji,
                count: r.count,
                viewer_reacted: r.reacted_by_me,
            })
            .collect();
        messages.push(MessageView {
            id: m.id,
            username,
            created_at: m.created_at,
            edited_at: m.edited_at,
            body: m.body,
            reactions,
            can_edit,
        });
    }

    // Sidebar data (mirrors get_room).
    let is_admin = user.role == "admin";
    let rooms = db::chat::list_rooms(&state.chat, &user.id, is_admin).await?;
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peers: Vec<User> = Vec::with_capacity(dm_rooms.len());
    for (_room, other_id) in &dm_rooms {
        if let Some(record) = db::auth::find_user_by_id(&state.auth, other_id).await? {
            dm_peers.push(record.into());
        }
    }

    let page = DmPage {
        user: &user,
        peer: &peer,
        room: &room,
        rooms: &rooms,
        dm_peers: &dm_peers,
        messages: &messages,
        asset_version: state.asset_version,
    };
    html(&page)
}
