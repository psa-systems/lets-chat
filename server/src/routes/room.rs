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
    ThreadPanelClosedFragment, ThreadPanelFragment,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
pub struct MessageForm {
    pub body: String,
    /// Orphan upload id from a prior `POST /api/upload`. When present, the
    /// row's `uploader_id` must equal the caller and `message_id` must still
    /// be NULL; the handler links the upload to the new message before
    /// broadcasting. The composer always submits this field (even empty), so
    /// the deserializer maps `""` -> None to keep blank sends working.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub file_id: Option<i64>,
}

fn empty_string_as_none<'de, D>(de: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<i64>().map(Some).map_err(serde::de::Error::custom),
    }
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
    let mut author_cache: HashMap<String, super::AuthorMeta> = HashMap::new();

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

    let reply_counts: HashMap<i64, i64> = db::chat::count_replies_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .collect();

    // Bulk-load attachments for the page in a single query.
    let message_ids: Vec<i64> = raw_messages.iter().map(|m| m.id).collect();
    let mut attachments_by_message =
        db::uploads::attachments_for_messages(&state.chat, &message_ids).await?;

    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    let mut prev: Option<(String, String)> = None;
    let mut unread_divider_placed = false;
    for m in raw_messages {
        let meta = if let Some(entry) = author_cache.get(&m.user_id) {
            entry.clone()
        } else {
            let entry = super::load_author_meta(&state, &m.user_id, &user.id).await?;
            author_cache.insert(m.user_id.clone(), entry.clone());
            entry
        };
        let can_edit = m.user_id == user.id;
        let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
        let reactions = reactions_by_message.remove(&m.id).unwrap_or_default();
        let attachments = attachments_by_message.remove(&m.id).unwrap_or_default();
        let is_follow_up = db::chat::is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        );
        prev = Some((m.user_id.clone(), m.created_at.clone()));
        let show_unread_divider =
            !unread_divider_placed && m.id > prior_watermark && m.user_id != user.id;
        if show_unread_divider {
            unread_divider_placed = true;
        }
        messages.push(MessageView {
            id: m.id,
            room_id: m.room_id,
            user_id: m.user_id.clone(),
            username: meta.username,
            display_name: meta.display_name,
            avatar_ext: meta.avatar_ext,
            status: meta.status,
            custom_status: meta.custom_status,
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
            reply_count: *reply_counts.get(&m.id).unwrap_or(&0),
            parent_id: m.parent_id,
            attachments,
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
    let (mut sidebar_rooms, sidebar_peers, switcher) =
        super::load_chrome(&state, &user, current_enclave).await?;
    if let Some(r) = sidebar_rooms.iter_mut().find(|r| r.id == room_id) {
        r.active = true;
    }

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
    let body = form.body.trim();
    // An attachment alone (no body) is a valid send (image-only messages);
    // both empty body AND no attachment is rejected.
    if body.is_empty() && form.file_id.is_none() {
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

    // For DMs, refuse the send if either user has blocked the other. The
    // peer is the other room member.
    if room.room_type == "dm" {
        let members = db::chat::list_room_member_ids(&state.chat, room_id).await?;
        if let Some(peer_id) = members.iter().find(|id| **id != user.id) {
            if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
                return Err(AppError::Forbidden);
            }
        }
    }

    // Validate any claimed attachment BEFORE the message insert so a stolen
    // file_id from another user can't slip a new message through.
    if let Some(file_id) = form.file_id {
        let (row, _) = db::uploads::get_upload(&state.chat, file_id)
            .await?
            .ok_or(AppError::BadRequest("unknown file_id".into()))?;
        if row.uploader_id != user.id || row.message_id.is_some() {
            return Err(AppError::Forbidden);
        }
    }

    let new_id = db::chat::insert_message(&state.chat, room_id, &user.id, body).await?;
    if let Some(file_id) = form.file_id {
        db::uploads::link_upload_to_message(&state.chat, file_id, new_id).await?;
    }
    super::touch_user_and_maybe_broadcast(&state, &user.id).await;

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
        parent_id: raw.parent_id,
    };

    let event = ChatEvent::NewMessage {
        message,
        is_dm: room.room_type == "dm",
    };
    state.hub.stop_typing(room_id, &user.id);
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
    let meta = super::load_author_meta(&state, &m.user_id, &user.id).await?;
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
    let reply_count = db::chat::count_replies(&state.chat, m.id).await?;
    let attachments = db::uploads::attachments_for_message(&state.chat, m.id).await?;
    let view = MessageView {
        id: m.id,
        room_id: m.room_id,
        user_id: m.user_id.clone(),
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
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
        reply_count,
        parent_id: m.parent_id,
        attachments,
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
    let meta = super::load_author_meta(&state, &m.user_id, &user.id).await?;
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
    let reply_count = db::chat::count_replies(&state.chat, m.id).await?;
    let attachments = db::uploads::attachments_for_message(&state.chat, m.id).await?;
    let view = MessageView {
        id: m.id,
        room_id: m.room_id,
        user_id: m.user_id.clone(),
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
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
        reply_count,
        parent_id: m.parent_id,
        attachments,
    };
    let fragment = SingleMessageFragment {
        message: &view,
        oob: false,
    };
    html(&fragment)
}

/// GET /room/:room_id/thread/:message_id - render the thread panel for a
/// parent message. The same handler serves DM rooms (room.room_type=='dm')
/// since access is gated by `is_room_accessible`.
pub async fn get_thread_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, message_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let parent = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if parent.room_id != room_id {
        return Err(AppError::NotFound);
    }
    if parent.parent_id.is_some() {
        return Err(AppError::BadRequest(
            "thread root cannot itself be a reply".into(),
        ));
    }

    let raw_replies = db::chat::list_thread_replies(&state.chat, message_id).await?;
    let mut author_cache: HashMap<String, super::AuthorMeta> = HashMap::new();
    let parent_meta = super::load_author_meta(&state, &parent.user_id, &user.id).await?;

    // Bulk-load attachments for the parent and every reply in a single query.
    let mut all_ids: Vec<i64> = raw_replies.iter().map(|r| r.id).collect();
    all_ids.push(parent.id);
    let mut attachments_by_message =
        db::uploads::attachments_for_messages(&state.chat, &all_ids).await?;

    let parent_view = MessageView {
        id: parent.id,
        room_id: parent.room_id,
        user_id: parent.user_id.clone(),
        username: parent_meta.username,
        display_name: parent_meta.display_name,
        avatar_ext: parent_meta.avatar_ext,
        status: parent_meta.status,
        custom_status: parent_meta.custom_status,
        created_at: parent.created_at.clone(),
        edited_at: parent.edited_at.clone(),
        body: parent.body.clone(),
        reactions: Vec::new(),
        can_edit: false,
        can_delete: false,
        viewer_id: user.id.clone(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        reply_count: 0,
        parent_id: None,
        attachments: attachments_by_message
            .remove(&parent.id)
            .unwrap_or_default(),
    };

    let mut replies: Vec<MessageView> = Vec::with_capacity(raw_replies.len());
    for r in raw_replies {
        let meta = if let Some(entry) = author_cache.get(&r.user_id) {
            entry.clone()
        } else {
            let entry = super::load_author_meta(&state, &r.user_id, &user.id).await?;
            author_cache.insert(r.user_id.clone(), entry.clone());
            entry
        };
        let attachments = attachments_by_message.remove(&r.id).unwrap_or_default();
        replies.push(MessageView {
            id: r.id,
            room_id: r.room_id,
            user_id: r.user_id,
            username: meta.username,
            display_name: meta.display_name,
            avatar_ext: meta.avatar_ext,
            status: meta.status,
            custom_status: meta.custom_status,
            created_at: r.created_at,
            edited_at: r.edited_at,
            body: r.body,
            reactions: Vec::new(),
            can_edit: false,
            can_delete: false,
            viewer_id: user.id.clone(),
            seen_caption: None,
            is_follow_up: false,
            show_unread_divider: false,
            reply_count: 0,
            parent_id: r.parent_id,
            attachments,
        });
    }

    let fragment = ThreadPanelFragment {
        room: &room,
        parent: &parent_view,
        replies: &replies,
    };
    html(&fragment)
}

/// POST /room/:room_id/thread/:parent_id/messages - post a reply into the
/// thread rooted at parent_id. Reuses the same access predicate as posting
/// a top-level message.
pub async fn post_thread_reply(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, parent_id)): Path<(i64, i64)>,
    axum::Form(form): axum::Form<MessageForm>,
) -> Result<Response, AppError> {
    let body = form.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }
    if user.is_banned || user.is_muted {
        return Err(AppError::Forbidden);
    }

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    if room.room_type == "dm" {
        let members = db::chat::list_room_member_ids(&state.chat, room_id).await?;
        if let Some(peer_id) = members.iter().find(|id| **id != user.id) {
            if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
                return Err(AppError::Forbidden);
            }
        }
    }

    let parent = db::chat::get_message(&state.chat, parent_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if parent.room_id != room_id {
        return Err(AppError::NotFound);
    }
    if parent.parent_id.is_some() {
        return Err(AppError::BadRequest(
            "thread root cannot itself be a reply".into(),
        ));
    }

    let new_id = db::chat::insert_reply(&state.chat, room_id, &user.id, body, parent_id).await?;
    super::touch_user_and_maybe_broadcast(&state, &user.id).await;

    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal("freshly inserted reply vanished".into()))?;
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
        parent_id: raw.parent_id,
    };
    let event = ChatEvent::ThreadReply { parent_id, message };
    state.hub.stop_thread_typing(room_id, parent_id, &user.id);
    super::broadcast_room_message(&state, &room, &event).await?;

    // Empty 204 - composer clears via hx-on::before-request, no body needed.
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// DELETE /thread-panel - close the panel by replacing it with an empty
/// hidden aside.
pub async fn close_thread_panel() -> Result<Html, AppError> {
    html(&ThreadPanelClosedFragment)
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
