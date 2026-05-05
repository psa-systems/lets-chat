use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::Message;
use crate::state::AppState;
use crate::views::room::{
    ComposerFragment, EditFormFragment, MessageView, ReactionView, RoomPage, SingleMessageFragment,
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
) -> Result<Response, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // Capture the viewer's watermark BEFORE marking-as-read at the end of
    // the handler, so the first message strictly above the watermark can
    // render an "Unread messages" divider.
    let prior_watermark = db::chat::get_dm_read_state(&state.chat, &user.id, room_id)
        .await?
        .map(|s| s.last_read_message_id)
        .unwrap_or(0);

    // Load messages, then resolve each author's username from the auth DB.
    // Cache lookups by user_id to avoid duplicate queries for the same author.
    let raw_messages = db::chat::list_messages(&state.chat, room_id).await?;
    let mut author_cache: HashMap<String, (String, Option<String>, Option<String>)> =
        HashMap::new();

    // Reactions for every message in the room, in a single query. Group them
    // by message_id so we can attach to each MessageView below.
    let mut reactions_by_message: HashMap<i64, Vec<ReactionView>> = HashMap::new();
    for (mid, r) in db::chat::list_room_reactions(&state.chat, room_id, &user.id).await? {
        reactions_by_message
            .entry(mid)
            .or_default()
            .push(ReactionView {
                emoji: r.emoji,
                count: r.count,
                viewer_reacted: r.reacted_by_me,
            });
    }

    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    let mut prev: Option<(String, String)> = None;
    let mut unread_divider_placed = false;
    for m in raw_messages {
        let (username, display_name, avatar_ext) = if let Some(entry) = author_cache.get(&m.user_id)
        {
            entry.clone()
        } else {
            let entry = match db::auth::find_user_by_id(&state.auth, &m.user_id).await? {
                Some(r) => (r.username, r.display_name, r.avatar_ext),
                None => ("(unknown)".to_string(), None, None),
            };
            author_cache.insert(m.user_id.clone(), entry.clone());
            entry
        };
        let can_edit = m.user_id == user.id;
        let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
        let reactions = reactions_by_message.remove(&m.id).unwrap_or_default();
        let is_follow_up = db::chat::is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        );
        prev = Some((m.user_id.clone(), m.created_at.clone()));
        // Place the unread divider above the first message strictly newer
        // than the prior watermark, ignoring own-authored messages so the
        // viewer's own send doesn't trigger a divider on their next visit.
        let show_unread_divider =
            !unread_divider_placed && m.id > prior_watermark && m.user_id != user.id;
        if show_unread_divider {
            unread_divider_placed = true;
        }
        messages.push(MessageView {
            id: m.id,
            user_id: m.user_id.clone(),
            username,
            display_name,
            avatar_ext,
            created_at: m.created_at,
            edited_at: m.edited_at,
            body: m.body,
            reactions,
            can_edit,
            can_delete,
            viewer_id: user.id.clone(),
            seen_caption: None,
            is_follow_up,
            show_unread_divider,
        });
    }

    // Mark the latest message as read for this viewer and notify other tabs
    // of the same user (and any other subscribers in the room) so badges
    // clear in real time. Skipped when the room has no messages.
    if let Some(last) = messages.last() {
        db::chat::set_last_read(&state.chat, &user.id, room_id, last.id).await?;
        let event = ChatEvent::DmRead {
            room_id,
            user_id: user.id.clone(),
            last_read_message_id: last.id,
            read_at: chrono::Utc::now().to_rfc3339(),
        };
        state.hub.broadcast_to_room(room_id, &event);
    }

    // Sidebar data (after marking-as-read so the badge for this room is 0).
    // Resolve the room's enclave so the switcher highlights the right icon
    // and the sidebar shows that enclave's rooms instead of DMs.
    let current_enclave = super::enclave_for_room(&state, room_id).await?;
    let (sidebar_rooms, sidebar_peers, switcher) =
        super::load_chrome(&state, &user, current_enclave).await?;

    let page = RoomPage {
        user: &user,
        room: &room,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        messages: &messages,
        asset_version: &state.asset_version,
    };
    let body = html(&page)?;
    let mut response = body.into_response();
    let (name, value) = crate::last_visited::set(&format!("/room/{room_id}"));
    response.headers_mut().insert(name, value);
    Ok(response)
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

    // Posting follows the same access predicate as reading. Site admins can
    // post in any non-DM room; DMs require explicit room membership for both
    // read and write. Enclave membership is required for non-DM rooms.
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
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
    super::broadcast_room_message(&state, &room, &event).await?;

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
    let (username, display_name, avatar_ext) =
        match db::auth::find_user_by_id(&state.auth, &m.user_id).await? {
            Some(r) => (r.username, r.display_name, r.avatar_ext),
            None => ("(unknown)".to_string(), None, None),
        };
    let can_edit = m.user_id == user.id;
    let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
    let reactions: Vec<ReactionView> = db::chat::list_reactions(&state.chat, m.id, &user.id)
        .await?
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id).await?;
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let view = MessageView {
        id: m.id,
        user_id: m.user_id.clone(),
        username,
        display_name,
        avatar_ext,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: user.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
    };
    let fragment = SingleMessageFragment {
        message: &view,
        oob: false,
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
    let (username, display_name, avatar_ext) =
        match db::auth::find_user_by_id(&state.auth, &m.user_id).await? {
            Some(r) => (r.username, r.display_name, r.avatar_ext),
            None => ("(unknown)".to_string(), None, None),
        };
    let reactions: Vec<ReactionView> = db::chat::list_reactions(&state.chat, m.id, &user.id)
        .await?
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id).await?;
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let view = MessageView {
        id: m.id,
        user_id: m.user_id.clone(),
        username,
        display_name,
        avatar_ext,
        created_at: m.created_at,
        edited_at: Some(edited_at_str),
        body: body.to_string(),
        reactions,
        can_edit: true,
        can_delete: true,
        viewer_id: user.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
    };
    let fragment = SingleMessageFragment {
        message: &view,
        oob: false,
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
    let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
    if !can_delete {
        return Err(AppError::Forbidden);
    }
    // Look up the next message in the room BEFORE soft-deleting so we can
    // detect whether deleting `m` exposes an orphaned follow-up. Capturing
    // the prior state of `m` here lets the regrouping decision use the
    // pre-delete grouping invariant.
    let next = db::chat::next_message_in_room(&state.chat, m.room_id, message_id).await?;

    db::moderation::soft_delete_message(&state.chat, message_id, &user.id).await?;

    if let Some(n) = next.as_ref() {
        let was_follow_up = db::chat::is_follow_up_of(
            Some((m.user_id.as_str(), m.created_at.as_str())),
            (n.user_id.as_str(), n.created_at.as_str()),
        );
        if was_follow_up {
            let regroup = ChatEvent::MessageRegrouped {
                message_id: n.id,
                room_id: m.room_id,
            };
            state.hub.broadcast_to_room(m.room_id, &regroup);
        }
    }

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
