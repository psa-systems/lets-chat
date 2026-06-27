use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::dm::DmPage;
use crate::views::home::WelcomePage;
use crate::views::html;
use crate::views::room::{MessageView, ReactionView};
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
) -> Result<Response, AppError> {
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
    let mut peer: User = peer_record.into();
    peer.status = super::effective_status(&state, &peer.id, &peer.status);

    // If either party has blocked the other, render Home with an inline error
    // bubble explaining why the conversation is unavailable. We deliberately
    // give the same message regardless of which direction the block was
    // placed so the blockee cannot probe whether the blocker has blocked
    // them specifically.
    let blocked = db::auth::is_blocked_either_way(&state.auth, &user.id, &peer.id).await?;
    if blocked {
        let (
            sidebar_categories,
            sidebar_starred_rooms,
            sidebar_starred_peers,
            sidebar_rooms,
            sidebar_peers,
            switcher,
            can_manage_sidebar_categories,
            sidebar_current_enclave,
        ) = super::load_chrome(&state, &user, None).await?;
        let msg = format!("You can't message @{}.", peer.username);
        let page = WelcomePage {
            user: &user,
            sidebar_categories: &sidebar_categories,
            sidebar_starred_rooms: &sidebar_starred_rooms,
            sidebar_starred_peers: &sidebar_starred_peers,
            can_manage_sidebar_categories,
            sidebar_current_enclave,
            sidebar_rooms: &sidebar_rooms,
            sidebar_peers: &sidebar_peers,
            switcher: &switcher,
            asset_version: &state.asset_version,
            flash_error: Some(&msg),
            pending_invites: 0,
        };
        return Ok(html(&page)?.into_response());
    }

    // Find or create the underlying DM room. The DM room is named after the
    // peer relative to the creator; the rendered title uses peer.username
    // directly so the displayed name matches whoever the viewer is talking to.
    let room = match db::chat::find_dm_room(&state.chat, &user.id, &peer.id).await? {
        Some(r) => r,
        None => {
            // A private profile cannot be DM'd by someone who has no prior
            // conversation with them. Render Home with an inline error bubble
            // so the user gets app chrome and a clear explanation rather than
            // a bare 404.
            if !peer.is_profile_public {
                let (
                    sidebar_categories,
                    sidebar_starred_rooms,
                    sidebar_starred_peers,
                    sidebar_rooms,
                    sidebar_peers,
                    switcher,
                    can_manage_sidebar_categories,
                    sidebar_current_enclave,
                ) = super::load_chrome(&state, &user, None).await?;
                let msg = format!(
                    "@{} has a private profile. You can't start a new DM with them.",
                    peer.username
                );
                let page = WelcomePage {
                    user: &user,
                    sidebar_categories: &sidebar_categories,
                    sidebar_starred_rooms: &sidebar_starred_rooms,
                    sidebar_starred_peers: &sidebar_starred_peers,
                    can_manage_sidebar_categories,
                    sidebar_current_enclave,
                    sidebar_rooms: &sidebar_rooms,
                    sidebar_peers: &sidebar_peers,
                    switcher: &switcher,
                    asset_version: &state.asset_version,
                    flash_error: Some(&msg),
                    pending_invites: 0,
                };
                return Ok(html(&page)?.into_response());
            }
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
    // Filter blocked authors. In a DM the only candidate is the peer; if
    // either party blocked the other we already returned above, so this is
    // strictly defensive.
    let blocked_authors = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let raw_messages: Vec<_> = db::chat::list_messages(&state.chat, room_id)
        .await?
        .into_iter()
        .filter(|m| !blocked_authors.contains(&m.user_id))
        .collect();
    let reply_counts: HashMap<i64, i64> = db::chat::count_replies_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .collect();
    let mut author_cache: HashMap<String, super::AuthorMeta> = HashMap::new();
    let message_ids: Vec<i64> = raw_messages.iter().map(|m| m.id).collect();
    let mut attachments_by_message =
        db::uploads::attachments_for_messages(&state.chat, &message_ids).await?;
    // LC-66: polls among these messages, so the loop skips build_poll_view
    // for ordinary messages.
    let poll_ids = db::polls::poll_message_ids(&state.chat, &message_ids).await?;
    // Bulk-load the set of currently-pinned message ids so the bubble's
    // hover menu shows Pin vs Unpin without an N+1 lookup.
    let pinned_ids = db::pinned::pinned_message_ids_for_room(&state.chat, room_id).await?;
    let bookmarked_ids =
        db::bookmarks::bookmarked_message_ids_in_room(&state.chat, &user.id, room_id).await?;
    // DMs have no enclave of their own but still resolve `:shortcode:` for
    // any enclave that opted into global sharing.
    let shared_emojis = db::custom_emojis::refs_globally_shared(&state.chat).await?;
    let quote_ids: Vec<i64> = raw_messages.iter().filter_map(|m| m.quote_id).collect();
    let quote_preview_map =
        crate::views::room::build_quote_previews_bulk(&state.chat, &state.auth, &quote_ids).await?;
    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    let mut prev: Option<(String, String)> = None;
    // LC-244: track the previous rendered message's UTC day for date separators.
    let mut prev_day: Option<String> = None;
    let mut unread_divider_placed = false;
    for m in raw_messages {
        let meta = if let Some(entry) = author_cache.get(&m.user_id) {
            entry.clone()
        } else {
            let entry = super::resolve_msg_author(
                &state,
                &m.user_id,
                m.webhook_id,
                m.email_inbox_id,
                m.bridge_id,
                m.bridge_foreign_name.as_deref(),
                m.bridge_kind.as_deref(),
                m.bridge_foreign_avatar.as_deref(),
                &user.id,
                m.room_id,
            )
            .await?;
            author_cache.insert(m.user_id.clone(), entry.clone());
            entry
        };
        let can_edit = m.user_id == user.id;
        let can_delete = m.user_id == user.id || user.role == "admin" || user.role == "moderator";
        let reaction_counts = db::chat::list_reactions(&state.chat, m.id, &user.id).await?;
        let reactor_titles = super::build_reactor_titles(&state, &reaction_counts).await;
        let reactions: Vec<ReactionView> = reaction_counts
            .into_iter()
            .zip(reactor_titles)
            .map(|(r, title)| {
                ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &shared_emojis)
            })
            .collect();
        let is_follow_up = db::chat::is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        );
        prev = Some((m.user_id.clone(), m.created_at.clone()));
        // LC-244: day separator above the first message and on each UTC day change.
        let cur_day = m.created_at.get(..10).unwrap_or("").to_string();
        let day_label = if prev_day.as_deref() != Some(cur_day.as_str()) {
            Some(crate::views::room::day_label_for(&m.created_at))
        } else {
            None
        };
        prev_day = Some(cur_day);
        let show_unread_divider =
            !unread_divider_placed && m.id > prior_watermark && m.user_id != user.id;
        if show_unread_divider {
            unread_divider_placed = true;
        }
        let attachments = attachments_by_message.remove(&m.id).unwrap_or_default();
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
            day_label,
            shame_enabled: false,
            shame_hidden: None,
            reply_count: *reply_counts.get(&m.id).unwrap_or(&0),
            parent_id: m.parent_id,
            attachments,
            mentions: Vec::new(),
            is_pinned: pinned_ids.contains(&m.id),
            is_bookmarked: bookmarked_ids.contains(&m.id),
            custom_emojis: shared_emojis.clone(),
            // LC-323: DMs have no enclave, so there are no #channel targets.
            channels: Vec::new(),
            quote_preview: m
                .quote_id
                .and_then(|qid| quote_preview_map.get(&qid).cloned()),
            suppress_quote_preview: false,
            is_system: m.is_system,
            poll: if poll_ids.contains(&m.id) {
                crate::views::room::build_poll_view(&state.chat, &state.auth, m.id, &user.id)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            },
            author_is_bot: meta.is_bot,
            actor: meta.actor.clone(),
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
        db::mentions::mark_mentions_read_for_room(&state.chat, &user.id, room_id, last.id).await?;
        let event = ChatEvent::DmRead {
            room_id,
            user_id: user.id.clone(),
            last_read_message_id: last.id,
            read_at: chrono::Utc::now().to_rfc3339(),
        };
        state.hub.broadcast_to_room(room_id, &event);
    }

    // Sidebar data (after marking-as-read so the badge for this DM is 0).
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        mut sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    if let Some(p) = sidebar_peers.iter_mut().find(|p| p.id == peer.id) {
        p.active = true;
    }

    let mute_mode = db::notifications::room_mute_mode(&state.chat, &user.id, room.id)
        .await
        .unwrap_or(db::notifications::MuteMode::None)
        .as_str()
        .to_string();

    // Pre-render the pinned strip. The DM strip's "See all" link uses
    // the dm-keyed URL so the navigation lands on `/dm/{peer_id}/pins`.
    let pin_path = format!("/dm/{}/pins", peer.id);
    let pinned_strip_html = super::pinned::build_strip_fragment(&state, room.id, pin_path, false)
        .await?
        .render()?;

    // LC-64: server-persisted draft restore + 60-day lazy cleanup, same
    // shape as the room handler. DMs share the `message_drafts` table
    // (keyed by user_id + room_id) because DMs are `room_type='dm'`
    // rows in the same `rooms` table.
    let initial_draft = db::drafts::get_fresh_or_purge(&state.chat, &user.id, room.id, 60)
        .await?
        .unwrap_or_default();

    let page = DmPage {
        user: &user,
        peer: &peer,
        room: &room,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        messages: &messages,
        asset_version: &state.asset_version,
        mute_mode,
        pinned_strip_html,
        initial_draft,
        max_upload_bytes: db::settings::max_upload_bytes(&state.settings).await,
        llm_available: state.llm_available(),
    };
    let body = html(&page)?;
    let mut response = body.into_response();
    let (name, value) = crate::last_visited::set(&format!("/dm/{}", peer.id));
    response.headers_mut().insert(name, value);
    Ok(response)
}
