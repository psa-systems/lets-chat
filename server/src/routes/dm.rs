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
use crate::ws::events::ChatEvent;

/// Extract the HH:MM portion from a timestamp string. Accepts both the
/// "YYYY-MM-DD HH:MM:SS" SQLite format and RFC3339 strings.
pub(crate) fn format_hhmm(read_at: &str) -> String {
    let after_date = read_at
        .split_once([' ', 'T'])
        .map(|(_, rest)| rest)
        .unwrap_or(read_at);
    after_date.chars().take(5).collect()
}

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
            let r = db::chat::create_dm_room(&state.chat, &dm_name, &user.id, &peer.id).await?;
            // Notify both parties so their sidebars pick up the new DM live.
            // Each user receives RoomMemberAdded for their own user_id; the WS
            // handler then re-renders that user's sidebar OOB.
            for member_id in [&user.id, &peer.id] {
                let event = ChatEvent::RoomMemberAdded {
                    room_id: r.id,
                    user_id: member_id.clone(),
                };
                state.hub.broadcast_to_user(member_id, &event);
            }
            r
        }
    };
    let room_id = room.id;

    // Defensive membership check. find_dm_room/create_dm_room should always
    // produce a room the current user is a member of, but verify so a stale
    // row can never bypass access control.
    if !db::chat::is_room_member(&state.chat, room_id, &user.id).await? {
        return Err(AppError::Forbidden);
    }

    // Capture the viewer's watermark BEFORE marking-as-read at the end of
    // the handler so we can render an "Unread messages" divider above the
    // first message strictly above the watermark.
    let prior_watermark = db::chat::get_dm_read_state(&state.chat, &user.id, room_id)
        .await?
        .map(|s| s.last_read_message_id)
        .unwrap_or(0);

    // Mirror routes/room.rs: load messages, resolve usernames, attach reactions.
    let raw_messages = db::chat::list_messages(&state.chat, room_id).await?;
    let mut username_cache: HashMap<String, String> = HashMap::new();
    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    let mut prev: Option<(String, String)> = None;
    let mut unread_divider_placed = false;
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
            user_id: m.user_id.clone(),
            username,
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

    // Compute the "Seen HH:MM" caption for the most recent own-authored
    // message that the peer has read. Gated symmetrically: both viewer and
    // peer must have read receipts enabled.
    if user.read_receipts_enabled && peer.read_receipts_enabled {
        if let Some((seen_msg_id, read_at)) =
            db::chat::find_dm_seen_state(&state.chat, room_id, &user.id, &peer.id).await?
        {
            let hhmm = format_hhmm(&read_at);
            if let Some(msg) = messages.iter_mut().find(|m| m.id == seen_msg_id) {
                msg.seen_caption = Some(hhmm);
            }
        }
    }

    // Mark the latest message as read for this viewer and broadcast to the
    // room so other tabs of this user clear the DM badge live. Skipped when
    // the DM has no messages yet.
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

    // Sidebar data (after marking-as-read so the badge for this DM is 0).
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;

    let page = DmPage {
        user: &user,
        peer: &peer,
        room: &room,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        messages: &messages,
        asset_version: &state.asset_version,
    };
    html(&page)
}
