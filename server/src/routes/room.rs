use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::{Message, User};
use crate::state::AppState;
use crate::views::room::{
    ComposerFragment, EditFormFragment, MessageView, RoomPage, SingleMessageFragment,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
pub struct MessageForm {
    pub body: String,
}

#[derive(Deserialize)]
pub struct EditMessageForm {
    pub body: String,
}

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
        let can_edit = m.user_id == user.id;
        messages.push(MessageView {
            id: m.id,
            username,
            created_at: m.created_at,
            edited_at: m.edited_at,
            body: m.body,
            // Reactions are populated in Task 11.
            reactions: Vec::new(),
            can_edit,
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

pub async fn post_message(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<MessageForm>,
) -> Result<Html, AppError> {
    // Reject blank submissions outright.
    let body = form.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }

    // Banned/muted users cannot post anywhere.
    if user.is_banned {
        return Err(AppError::Forbidden);
    }
    if user.is_muted {
        return Err(AppError::Forbidden);
    }

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Posting access check. Public rooms accept any authenticated, non-banned,
    // non-muted user. DM and private rooms require room membership. Admin
    // status alone does not grant posting rights to a DM.
    let can_post = match room.room_type.as_str() {
        "public" => true,
        _ => db::chat::is_room_member(&state.chat, room_id, &user.id).await?,
    };
    if !can_post {
        return Err(AppError::Forbidden);
    }

    let new_id = db::chat::insert_message(&state.chat, room_id, &user.id, body).await?;

    // Re-fetch the inserted row to pick up the server-assigned created_at.
    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal(
            "freshly inserted message vanished".into(),
        ))?;

    // Resolve author display name from the auth DB.
    let author_name = db::auth::find_user_by_id(&state.auth, &raw.user_id)
        .await?
        .map(|r| r.username)
        .unwrap_or_else(|| "(unknown)".to_string());

    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name,
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
    };

    let event = ChatEvent::NewMessage {
        message,
        is_dm: room.room_type == "dm",
    };
    state.hub.broadcast_to_room(room_id, &event);

    let fragment = ComposerFragment { room: &room };
    html(&fragment)
}

/// GET /messages/:id/edit - return the inline edit form for a message.
/// Only the author may request the edit form.
pub async fn get_edit_form(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if m.user_id != user.id {
        return Err(AppError::Forbidden);
    }
    let fragment = EditFormFragment {
        message_id,
        current_body: &m.body,
    };
    html(&fragment)
}

/// GET /messages/:id - return a single message rendered as a fragment.
/// Used as the Cancel target from the edit form.
pub async fn get_single_message(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let username = db::auth::find_user_by_id(&state.auth, &m.user_id)
        .await?
        .map(|r| r.username)
        .unwrap_or_else(|| "(unknown)".to_string());
    let can_edit = m.user_id == user.id;
    let view = MessageView {
        id: m.id,
        username,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        // Reactions are populated in Task 11.
        reactions: Vec::new(),
        can_edit,
    };
    let fragment = SingleMessageFragment {
        message: &view,
        can_edit,
    };
    html(&fragment)
}

/// PATCH /messages/:id - update the body of a message. Only the author may edit.
pub async fn patch_message(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    axum::Form(form): axum::Form<EditMessageForm>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if m.user_id != user.id {
        return Err(AppError::Forbidden);
    }
    let body = form.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("empty body".into()));
    }

    let edited_at_str = db::chat::update_message_body(&state.chat, message_id, body).await?;

    let event = ChatEvent::MessageEdited {
        message_id,
        room_id: m.room_id,
        new_body: body.to_string(),
        edited_at: edited_at_str.clone(),
    };
    state.hub.broadcast_to_room(m.room_id, &event);

    // Render the updated message as a single-message fragment so the sender's
    // edit form is replaced inline.
    let username = db::auth::find_user_by_id(&state.auth, &m.user_id)
        .await?
        .map(|r| r.username)
        .unwrap_or_else(|| "(unknown)".to_string());
    let view = MessageView {
        id: m.id,
        username,
        created_at: m.created_at,
        edited_at: Some(edited_at_str),
        body: body.to_string(),
        // Reactions are populated in Task 11.
        reactions: Vec::new(),
        can_edit: true,
    };
    let fragment = SingleMessageFragment {
        message: &view,
        can_edit: true,
    };
    html(&fragment)
}

/// DELETE /messages/:id - soft-delete a message.
/// Author, admins, and moderators may delete.
pub async fn delete_message(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let can_delete =
        m.user_id == user.id || user.role == "admin" || user.role == "moderator";
    if !can_delete {
        return Err(AppError::Forbidden);
    }
    db::moderation::soft_delete_message(&state.chat, message_id, &user.id).await?;
    let event = ChatEvent::MessageDeleted {
        message_id,
        room_id: m.room_id,
    };
    state.hub.broadcast_to_room(m.room_id, &event);
    // Return the deleted-fragment HTML directly so the requesting tab also updates.
    let body = format!(
        "<div id=\"msg-{}\" class=\"px-4 py-2 italic text-slate-400\">[deleted]</div>",
        message_id
    );
    Ok(axum::response::Html(body).into_response())
}
