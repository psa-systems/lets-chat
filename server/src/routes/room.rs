use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::{Message, User};
use crate::state::AppState;
use crate::views::room::{
    ComposerQuoteChip, EditFormFragment, HistoryEntryView, HistoryPanelClosedFragment,
    HistoryPanelFragment, MessageView, ReactionView, RoomPage, SingleMessageFragment,
    ThreadPanelClosedFragment, ThreadPanelFragment,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// LC-153: upper bound on a message body, counted in characters so multibyte
/// (CJK, emoji) text is not unfairly penalized. Generous for chat use while
/// stopping ~2 MiB bodies from being stored and broadcast to every subscriber
/// (the send path otherwise only checked for emptiness). Applies to new
/// messages, thread replies, and edits.
pub(crate) const MAX_MESSAGE_CHARS: usize = 16_000;

/// Reject an over-length body before any DB write or broadcast. `body` is the
/// already-trimmed message text. `pub(crate)` so the JSON API v1 message POST
/// (`routes::api`) enforces the same cap as the web composer.
pub(crate) fn check_message_length(body: &str) -> Result<(), AppError> {
    if body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest(format!(
            "message too long (max {MAX_MESSAGE_CHARS} characters)"
        )));
    }
    Ok(())
}

/// LC-276: composer Write/Preview toggle.
#[derive(Deserialize)]
pub struct PreviewForm {
    #[serde(default)]
    pub body: String,
}

/// `POST /room/{room_id}/preview`
///
/// Render the composer draft through the SAME markdown pipeline that posting
/// uses, so the Write/Preview toggle shows exactly what will appear. Access-
/// gated (403 on a room the caller cannot see). Mentions are NOT resolved (the
/// message is not posted, so no mention rows exist) - `@user` renders as plain
/// text; this is a formatting preview, not a mention-resolution preview. The
/// body is capped to `MAX_MESSAGE_CHARS` before rendering to honor LC-153 (the
/// markdown render runs synchronously on the request thread).
pub async fn post_preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<PreviewForm>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let capped: String = form.body.chars().take(MAX_MESSAGE_CHARS).collect();
    let emojis = db::custom_emojis::refs_for_room(&state.chat, room_id).await?;
    let rendered = crate::views::markdown::render(&capped, &[], &emojis);
    Ok(Html(format!(
        r#"<div class="lc-md leading-snug">{rendered}</div>"#
    )))
}

/// `GET /messages/{message_id}/raw`
///
/// LC-282: the raw markdown source of a message, as `text/plain`, for the
/// "Copy text" hover action. Fetched on click (not inlined per row) to keep
/// page weight flat. 404 for a missing/soft-deleted message; access-gated to
/// the message's room. Returns only what the caller can already see.
pub async fn get_message_raw(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Response, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, m.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    Ok(m.body.into_response())
}

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
    /// Optional id of the earlier message this send is quote-replying to.
    /// Same blank-string handling as `file_id`: the composer always submits
    /// the field (so empty deserializes as `None`).
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub quote_id: Option<i64>,
    /// LC-230: opaque client-generated id (UUID) for optimistic local echo.
    /// Echoed back on the author's WS broadcast as `data-lc-client-id` so the
    /// sending tab can dedupe its optimistic placeholder against the
    /// canonical render. Never persisted. Sanitized in `post_message`: only
    /// `[A-Za-z0-9-]`, max 64 chars; anything else is treated as absent
    /// rather than rejected (the echo is a UX nicety and must never block a
    /// send).
    #[serde(default)]
    pub client_id: Option<String>,
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

/// Render the WebRTC voice-channel page for an enclave `voice` room. The
/// initial participant list comes from the hub's live presence map; the
/// browser mesh keeps it in sync from there.
async fn get_voice_room(
    state: &AppState,
    user: &User,
    room: &crate::models::Room,
) -> Result<Response, AppError> {
    let enclave_id = super::enclave_for_room(state, room.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, Some(enclave_id)).await?;
    let mut participants = Vec::new();
    for uid in state.hub.voice_room_users(room.id) {
        let label = db::auth::find_user_by_id(&state.auth, &uid)
            .await?
            .map(|r| {
                r.display_name
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(r.username)
            })
            .unwrap_or_else(|| "(unknown)".to_string());
        participants.push(crate::views::voice::VoiceParticipant {
            user_id: uid,
            label,
        });
    }
    let page = crate::views::voice::VoicePage {
        user,
        room,
        enclave_id,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        ice_servers: &state.ice_servers,
        participants: &participants,
    };
    Ok(html(&page)?.into_response())
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

    // Voice channels render a WebRTC mesh page instead of a message list.
    if room.is_voice {
        return get_voice_room(&state, &user, &room).await;
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
    // Filter out messages authored by anyone the viewer has blocked (or who
    // has blocked the viewer) so the room hides them everywhere, not just in
    // DMs.
    let blocked_authors = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let raw_messages: Vec<_> = db::chat::list_messages(&state.chat, room_id)
        .await?
        .into_iter()
        .filter(|m| !blocked_authors.contains(&m.user_id))
        .collect();
    let mut author_cache: HashMap<String, super::AuthorMeta> = HashMap::new();

    // Load the enclave's custom emoji set once for the whole page. Reactions
    // and message bodies share the same map, so a single lookup serves both.
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, room_id).await?;

    // Reactions for every message in the room, in a single query. Group them
    // by message_id so we can attach to each MessageView below. LC-266: resolve
    // every reactor's display name for the whole page in ONE auth query, then
    // build each pill's "who reacted" title from that map.
    let room_reactions = db::chat::list_room_reactions(&state.chat, room_id, &user.id).await?;
    let reaction_only: Vec<crate::models::reaction::Reaction> =
        room_reactions.iter().map(|(_, r)| r.clone()).collect();
    let reactor_titles = super::build_reactor_titles(&state, &reaction_only).await;
    let mut reactions_by_message: HashMap<i64, Vec<ReactionView>> = HashMap::new();
    for ((mid, r), title) in room_reactions.into_iter().zip(reactor_titles) {
        reactions_by_message
            .entry(mid)
            .or_default()
            .push(ReactionView::new(
                r.emoji,
                r.count,
                r.reacted_by_me,
                title,
                &custom_emojis,
            ));
    }

    let reply_counts: HashMap<i64, i64> = db::chat::count_replies_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .collect();

    // Bulk-load attachments for the page in a single query.
    let message_ids: Vec<i64> = raw_messages.iter().map(|m| m.id).collect();
    let mut attachments_by_message =
        db::uploads::attachments_for_messages(&state.chat, &message_ids).await?;
    // LC-66: which message ids are polls, so the loop only builds poll views
    // for actual polls (one query instead of a per-message probe).
    let poll_ids = db::polls::poll_message_ids(&state.chat, &message_ids).await?;

    // Bulk-load mention rows so each MessageView can render @username chips
    // without an N+1 query.
    let mut mentions_by_message =
        db::mentions::mentions_for_messages(&state.chat, &state.auth, &message_ids).await?;

    // Bulk-load the set of currently-pinned message ids so the bubble's
    // hover menu shows Pin vs Unpin without an N+1 lookup.
    let pinned_ids = db::pinned::pinned_message_ids_for_room(&state.chat, room_id).await?;
    // Same shape for the viewer's bookmarks in this room - per-viewer state.
    let bookmarked_ids =
        db::bookmarks::bookmarked_message_ids_in_room(&state.chat, &user.id, room_id).await?;

    // Resolve quote-reply previews for every message that quotes one, in a
    // single bulk lookup keyed by the quoted message's id.
    let quote_ids: Vec<i64> = raw_messages.iter().filter_map(|m| m.quote_id).collect();
    let quote_preview_map =
        crate::views::room::build_quote_previews_bulk(&state.chat, &state.auth, &quote_ids).await?;

    // Resolve the viewer's effective mod power for this room once, not
    // per-message. The override lives in `room_role_overrides` and never
    // changes mid-render.
    let viewer_is_room_mod =
        db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await?;

    let mut messages: Vec<MessageView> = Vec::with_capacity(raw_messages.len());
    let mut prev: Option<(String, String)> = None;
    // LC-244: track the previous rendered message's UTC day so a day change
    // (or the first message) places a date separator above the row.
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
            )
            .await?;
            author_cache.insert(m.user_id.clone(), entry.clone());
            entry
        };
        let can_edit = m.user_id == user.id;
        let can_delete = m.user_id == user.id || viewer_is_room_mod;
        let reactions = reactions_by_message.remove(&m.id).unwrap_or_default();
        let attachments = attachments_by_message.remove(&m.id).unwrap_or_default();
        let mentions = mentions_by_message.remove(&m.id).unwrap_or_default();
        let is_follow_up = db::chat::is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        );
        prev = Some((m.user_id.clone(), m.created_at.clone()));
        // LC-244: day separator above the first message and on each UTC day
        // change. `created_at` is "YYYY-MM-DD HH:MM:SS"; the date is its first
        // 10 chars.
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
            reply_count: *reply_counts.get(&m.id).unwrap_or(&0),
            parent_id: m.parent_id,
            attachments,
            mentions,
            is_pinned: pinned_ids.contains(&m.id),
            is_bookmarked: bookmarked_ids.contains(&m.id),
            custom_emojis: custom_emojis.clone(),
            quote_preview: m
                .quote_id
                .and_then(|qid| quote_preview_map.get(&qid).cloned()),
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

    // Mark the latest message as read for this viewer and notify other tabs
    // of the same user (and any other subscribers in the room) so badges
    // clear in real time. Skipped when the room has no messages.
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

    // Sidebar data (after marking-as-read so the badge for this room is 0).
    // Resolve the room's enclave so the switcher highlights the right icon
    // and the sidebar shows that enclave's rooms instead of DMs.
    let current_enclave = super::enclave_for_room(&state, room_id).await?;
    // LC-143: remember this as the enclave's last-opened room so clicking the
    // enclave reopens it. Best-effort; a failed write must not break the view.
    if let Some(eid) = current_enclave {
        if let Err(e) = db::enclave::set_last_room(&state.chat, &user.id, eid, room_id).await {
            tracing::warn!(error = %e, "set_last_room failed");
        }
    }
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        mut sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, current_enclave).await?;
    let mut sidebar_categories = sidebar_categories;
    if let Some(r) = sidebar_rooms.iter_mut().find(|r| r.id == room_id) {
        r.active = true;
    } else {
        // After the LC-79 redesign categorized rooms live inside
        // `sidebar_categories[i].rooms`; scan those too so the active
        // room's link still highlights when it sits in a category.
        for cat in sidebar_categories.iter_mut() {
            if let Some(r) = cat.rooms.iter_mut().find(|r| r.id == room_id) {
                r.active = true;
                break;
            }
        }
    }

    let mute_mode = db::notifications::room_mute_mode(&state.chat, &user.id, room_id)
        .await
        .unwrap_or(db::notifications::MuteMode::None)
        .as_str();

    // LC-84: header link "Moderators" only rendered for callers who can
    // grant/revoke per-room role overrides. Same authority as
    // `routes::room_rbac::require_can_manage`: site admin, or enclave
    // owner/admin for the room's enclave.
    let can_manage_overrides = {
        let enclave_role = if let Some(eid) = current_enclave {
            db::enclave::get_membership(&state.chat, eid, &user.id)
                .await?
                .map(|m| m.role)
        } else {
            None
        };
        crate::perms::room_can_manage_overrides(enclave_role, &user.role)
    };

    // LC-85: whether to render the composer vs a read-only notice.
    let can_post = can_post_with_policy(&state, &user, room_id, &room.posting_allowed_for).await?;
    let posting_locked_reason = if can_post {
        ""
    } else {
        match room.posting_allowed_for.as_str() {
            "moderators_only" => "Only moderators can post in this room.",
            "admins_only" => "Only admins can post in this room.",
            _ => "",
        }
    };

    // Pre-render the pinned strip so the page template can inline it
    // verbatim. Builder resolves author labels in a single bulk auth
    // lookup (see `routes::pinned::resolve_author_labels`).
    let pin_path = format!("/room/{room_id}/pins");
    let pinned_strip_html = super::pinned::build_strip_fragment(&state, room_id, pin_path, false)
        .await?
        .render()?;

    // LC-64: server-persisted draft restore + 60-day lazy cleanup.
    // The helper deletes the row in the same call if it is stale, so
    // the user sees an empty composer (and the row is gone) without a
    // background sweep.
    let initial_draft = db::drafts::get_fresh_or_purge(&state.chat, &user.id, room_id, 60)
        .await?
        .unwrap_or_default();

    let page = RoomPage {
        user: &user,
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
        can_manage_overrides,
        can_post,
        posting_locked_reason,
        initial_draft: &initial_draft,
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
) -> Result<Response, AppError> {
    let body = form.body.trim();
    // An attachment alone (no body) is a valid send (image-only messages);
    // both empty body AND no attachment is rejected.
    if body.is_empty() && form.file_id.is_none() {
        return Err(AppError::BadRequest("message body cannot be empty".into()));
    }
    check_message_length(body)?;

    // Banned/muted users cannot post anywhere.
    if user.is_banned {
        return Err(AppError::Forbidden);
    }
    if user.is_muted {
        return Err(AppError::Forbidden);
    }

    // LC-94: per-user message rate limit. Cap is read from the
    // settings KV (`rate_limit_messages`); '0' (or missing) = the
    // feature is off, which is the safe default. The check runs
    // before any DB work other than the cheap setting fetch so a
    // spammer cannot make the server thrash on room/access lookups.
    let msg_cap = crate::rate_limit::read_u32_setting(&state.settings, "rate_limit_messages").await;
    if let crate::rate_limit::Outcome::Deny { retry_after } =
        state
            .rate_limits
            .check(crate::rate_limit::RateLimitKind::Message, &user.id, msg_cap)
    {
        return Err(AppError::TooManyRequests(
            format!("you are sending messages too quickly; retry in {retry_after} seconds"),
            retry_after,
        ));
    }

    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // LC-217: per-enclave message rate-limit override. Layered AFTER the
    // global check so a non-zero per-enclave burst tightens the cap for
    // posts inside this enclave without relaxing the global. Separate
    // counter key (`enc:{eid}` prefix) so in-enclave activity is tracked
    // independently of the global counter; the user's global counter was
    // already incremented by the call above. DMs (enclave_id None) and
    // rooms whose enclave has the override at 0 fall through.
    let (enclave_id_opt, enclave_burst) =
        db::enclave::get_msg_rate_limit_burst_for_room(&state.chat, room_id).await?;
    if let Some(eid) = enclave_id_opt {
        if enclave_burst > 0 {
            let key = format!("enc:{eid}:{}", user.id);
            if let crate::rate_limit::Outcome::Deny { retry_after } = state.rate_limits.check(
                crate::rate_limit::RateLimitKind::Message,
                &key,
                enclave_burst,
            ) {
                return Err(AppError::TooManyRequests(
                    format!("you are sending messages too quickly in this enclave; retry in {retry_after} seconds"),
                    retry_after,
                ));
            }
        }
    }

    // Posting follows the same access predicate as reading. Site admins can
    // post in any non-DM room; DMs require explicit room membership for both
    // read and write. Enclave membership is required for non-DM rooms.
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    // LC-85: read-only / moderators-only rooms gate sends on the
    // caller's effective role (LC-84). The compose box client-side
    // already disables for callers below the bar; this server check
    // catches forged POSTs.
    if !can_post_with_policy(&state, &user, room_id, &room.posting_allowed_for).await? {
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
        // LC-93: per-enclave quota. The user's own quota was already
        // enforced at upload time; this catches the case where the
        // upload fit under the user's cap but attaching it to a room
        // in this enclave would push the enclave over its cap. DMs
        // (`enclave_id IS NULL`) have no enclave quota and skip.
        // SUM + insert is intentionally non-transactional for the same
        // reason as the upload-time user check: simultaneous attaches
        // can both pass and overshoot by at most one attachment's
        // bytes, which is fine for self-hosted volumes.
        if let Some(enclave_id) = db::quota::enclave_id_for_room(&state.chat, room_id).await? {
            if let Some(quota) = db::quota::get_enclave_quota(&state.chat, enclave_id).await? {
                let used = db::quota::sum_enclave_usage(&state.chat, enclave_id).await?;
                if used.saturating_add(row.size_bytes) > quota {
                    return Err(AppError::PayloadTooLarge(format!(
                        "attachment would exceed the enclave storage quota \
                         ({quota} bytes; using {used}, this attachment is {})",
                        row.size_bytes
                    )));
                }
            }
        }
    }

    // LC-94: link filter. Walk the body's URLs and check each host
    // against the admin's deny-list rules. The `find_match` helper
    // returns the first hit; `block` rejects the send up-front,
    // `quarantine` defers post-insert handling until we have a
    // message id, and `warn` falls through to a normal send + audit
    // log entry. Skipped entirely when the toggle is off, so the
    // common case pays a single setting fetch.
    let link_filter_on = db::settings::get_setting(&state.settings, "link_filter_enabled")
        .await?
        .as_deref()
        == Some("true");
    let link_decision = if link_filter_on {
        db::anti_spam::find_match(&state.chat, &crate::links::extract_hosts(body)).await?
    } else {
        None
    };
    if let Some((rule, _)) = &link_decision {
        if rule.action == db::anti_spam::FilterAction::Block {
            return Err(AppError::BadRequest(format!(
                "this message contains a link to a disallowed domain ({})",
                rule.pattern
            )));
        }
    }

    // Validate the quoted message (if any) before the insert. The quoted row
    // must exist, live in the same room as this send, not itself be a thread
    // reply (we don't quote inside threads), and not be the message currently
    // being inserted (impossible by ordering, but explicitly rejected for
    // future-proofing against any retry/idempotency-key flow).
    // LC-76: slash commands. Runs after every access / posting gate above,
    // so a command can never bypass room RBAC. An unknown `/foo` returns
    // None and falls through to posting the literal text.
    if body.starts_with('/') {
        if let Some(resp) = super::slash::try_dispatch(&state, &room, &user, body).await? {
            return Ok(resp);
        }
    }

    if let Some(qid) = form.quote_id {
        let quoted = db::chat::get_message(&state.chat, qid)
            .await?
            .ok_or(AppError::BadRequest("unknown quote_id".into()))?;
        if quoted.room_id != room_id {
            return Err(AppError::BadRequest(
                "quoted message is in a different room".into(),
            ));
        }
        if quoted.parent_id.is_some() {
            return Err(AppError::BadRequest(
                "cannot quote a thread reply; quote the thread root instead".into(),
            ));
        }
    }

    let new_id =
        db::chat::insert_message_quoted(&state.chat, room_id, &user.id, body, form.quote_id)
            .await?;
    if let Some(file_id) = form.file_id {
        db::uploads::link_upload_to_message(&state.chat, file_id, new_id).await?;
    }
    super::touch_user_and_maybe_broadcast(&state, &user.id).await;

    // LC-94: handle the link-filter decision now that we have a
    // message id. `quarantine` flips the row's quarantined bit and
    // records the catch in `link_filter_quarantine`, then returns
    // the empty composer fragment without broadcasting - the message
    // is invisible to everyone (including the author on a refresh,
    // since `list_messages` filters quarantined rows) until an admin
    // reviews. `warn` lets the message through but writes an audit
    // entry the admin can see in the mod log.
    if let Some((rule, host)) = &link_decision {
        match rule.action {
            db::anti_spam::FilterAction::Block => unreachable!("blocked above"),
            db::anti_spam::FilterAction::Quarantine => {
                sqlx::query("UPDATE messages SET quarantined = 1 WHERE id = ?")
                    .bind(new_id)
                    .execute(&state.chat)
                    .await?;
                db::anti_spam::insert_quarantine(&state.chat, new_id, &rule.pattern, host).await?;
                db::moderation::log_mod_action(
                    &state.chat,
                    "link_quarantine",
                    &user.id,
                    "system",
                    Some(&rule.pattern),
                    Some(room_id),
                    Some(host),
                )
                .await?;
                // LC-228: quarantine holds the message silently. Form is
                // `hx-swap="none"` so any returned body is discarded;
                // return NO_CONTENT to skip the ~5 KB ComposerFragment
                // render. (Pre-LC-228 returned a rendered fragment for
                // nothing; same byte-on-wire + CPU cost as the success
                // path.)
                //
                // LC-230: this is the one 2xx path that never broadcasts a
                // NewMessage, so the author's optimistic placeholder would
                // sit as "Sending..." forever. The X-LC-Echo-Drop header
                // tells the composer to remove the placeholder. Removal
                // (not a "held for review" label) keeps quarantine's
                // existing posture: the filter's existence is not revealed
                // to the poster.
                let mut response = StatusCode::NO_CONTENT.into_response();
                response.headers_mut().insert(
                    axum::http::header::HeaderName::from_static("x-lc-echo-drop"),
                    axum::http::HeaderValue::from_static("1"),
                );
                return Ok(response);
            }
            db::anti_spam::FilterAction::Warn => {
                db::moderation::log_mod_action(
                    &state.chat,
                    "link_warn",
                    &user.id,
                    "system",
                    Some(&rule.pattern),
                    Some(room_id),
                    Some(host),
                )
                .await?;
            }
        }
    }

    // LC-230: sanitize the optimistic-echo client id (drop, never reject).
    let client_id = form.client_id.as_deref().map(str::trim).filter(|s| {
        !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });

    finalize_message_send(&state, &room, &user, new_id, body, client_id).await?;

    // LC-228: form is `hx-swap="none"` (composer.html:11), so any returned
    // body is discarded by htmx. Skip the ~5 KB ComposerFragment render +
    // serialization and return 204. The user-visible effects already
    // happened: finalize_message_send fanned the rendered message out to
    // every connection via the WS hub, and hx-on::after-request clears the
    // textarea + disables the send button based on `event.detail.successful`
    // (true for any 2xx including 204).
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Post-insert side effects shared by the live POST handler and the
/// scheduled-message dispatcher. Re-fetches the row to pick up the
/// server-assigned `created_at`, broadcasts the rendered fragment via the
/// hub, then reconciles mentions and fans out per-user notifications
/// (including the DM implicit-mention path). Returns the constructed
/// `Message` so callers can log or attach it to their own response.
pub(crate) async fn finalize_message_send(
    state: &AppState,
    room: &crate::models::Room,
    author: &User,
    new_id: i64,
    body: &str,
    // LC-230: optimistic-echo dedupe id from the composer; `None` for every
    // non-composer caller (slash, polls, API, scheduled, reply-by-email).
    client_id: Option<&str>,
) -> Result<Message, AppError> {
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
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };

    let event = ChatEvent::NewMessage {
        message: message.clone(),
        is_dm: room.room_type == "dm",
        client_id: client_id.map(str::to_string),
    };
    state.hub.stop_typing(room.id, &author.id);
    super::broadcast_room_message(state, room, &event).await?;

    // LC-75: publish to outgoing webhooks (best-effort, never blocks the post).
    // LC-78: include the `actor` block so bridge daemons can self-filter.
    crate::outgoing::enqueue(
        &state.chat,
        "message.posted",
        room.id,
        serde_json::json!({
            "message_id": message.id,
            "user_id": message.user_id,
            "author": message.author_name,
            "body": message.body,
            "actor": super::outgoing_actor(&message.user_id, None, None, None, None),
        }),
    )
    .await;

    // Mention extraction + fan-out. Only for non-DM rooms (DMs are implicit
    // pings - we emit a Mentioned event below without writing mention rows).
    // Token resolution covers `@username`, `@here`, and `@channel` via the
    // shared resolver; broadcast tokens write one row per resolved user.
    if room.room_type != "dm" {
        let targets = resolve_targets_for_body(state, room, &author.id, body).await?;
        if !targets.is_empty() {
            let (added, _removed) = db::mentions::reconcile_mentions(
                &state.chat,
                new_id,
                room.id,
                &author.id,
                &targets,
            )
            .await?;
            let snippet = build_snippet(body);
            let author_label = author_label(author);
            let events: Vec<(String, ChatEvent)> = added
                .iter()
                .map(|t| {
                    (
                        t.user_id.clone(),
                        ChatEvent::Mentioned {
                            kind: "mention".into(),
                            room_id: room.id,
                            room_type: room.room_type.clone(),
                            room_label: format!("#{}", room.name),
                            message_id: new_id,
                            mentioned_user_id: t.user_id.clone(),
                            author_label: author_label.clone(),
                            snippet: snippet.clone(),
                            target_path: format!("/room/{}", room.id),
                        },
                    )
                })
                .collect();
            fanout_mention_events(state, room, events).await;
        }
    } else {
        // DM: implicit mention. Notify the peer regardless of subscription
        // state. No mention row is written; DM unread state already
        // governs read tracking.
        let members = db::chat::list_room_member_ids(&state.chat, room.id).await?;
        if let Some(peer_id) = members.into_iter().find(|id| id != &author.id) {
            let snippet = build_snippet(body);
            let author_label = author_label(author);
            let event = ChatEvent::Mentioned {
                kind: "dm".into(),
                room_id: room.id,
                room_type: "dm".into(),
                room_label: author_label.clone(),
                message_id: new_id,
                mentioned_user_id: peer_id.clone(),
                author_label,
                snippet,
                target_path: format!("/dm/{}", author.id),
            };
            state.hub.broadcast_to_user(&peer_id, &event);
            // Mute is enforced downstream: the WS `Mentioned` arm and
            // `push::dispatch` both consult `room_mute_mode(peer_id,
            // dm_room_id)` and drop the event when the peer has muted
            // this DM.
            crate::push::dispatch(state, &peer_id, &event).await;
            // LC-77-REPLY: per-DM email notification with Reply-To. Fire-
            // and-forget so the post handler does not wait on SMTP. The
            // dispatcher's gates (verified email + opt-in + rate limit)
            // are the actual filtering surface.
            let send_state = state.clone();
            let peer_id_for_email = peer_id.clone();
            let room_id = room.id;
            let room_name = room.name.clone();
            let message_id_for_email = new_id;
            tokio::spawn(async move {
                let _ = crate::email::notification::dispatch_mention_notification(
                    &send_state,
                    &peer_id_for_email,
                    message_id_for_email,
                    crate::email::notification::NotificationKind::Dm,
                    room_id,
                    &room_name,
                )
                .await;
            });
        }
    }

    // LC-64: clear the sent author's draft for this room. Single
    // chokepoint covers both direct sends and LC-62 scheduled-message
    // deliveries (the dispatcher funnels through here). Best-effort:
    // a DB hiccup on the draft delete must not fail the send.
    // LC-239: when a draft actually went away, clear its sidebar indicator
    // across the author's tabs. Gated on rows_affected so the common
    // no-draft send does not fan a redundant OOB to every tab.
    if db::drafts::delete(&state.chat, &author.id, room.id)
        .await
        .unwrap_or(0)
        > 0
    {
        state.hub.broadcast_to_user(
            &author.id,
            &ChatEvent::DraftChanged {
                user_id: author.id.clone(),
                room_id: room.id,
                has_draft: false,
            },
        );
    }

    Ok(message)
}

/// LC-77: broadcast + fan out a message authored by an email-ingress inbox.
/// Email-shaped sibling of [`finalize_webhook_message_send`]. Same posture:
/// empty user_id, `email_inbox_id` set, inbox name as the author label, no
/// author to exclude from mentions.
///
/// **Named decision (NOT inherited from LC-74 silently): the link filter
/// (link_filter_quarantine block in `post_message`) is skipped for
/// email-ingress messages, mirroring the webhook posture.** The
/// secret-gates-it trust model is consistent: the admin who issued the
/// ingress address authorized that channel to post. External senders
/// forwarding into the inbox carry unvetted links, but the same is true
/// of webhooks (an admin-configured webhook may carry an upstream link
/// the admin didn't vet content-by-content). If the link-filter posture
/// needs to change for email specifically, revisit at that point; it is
/// a deliberate choice here, not drift.
pub(crate) async fn finalize_email_inbox_message_send(
    state: &AppState,
    room: &crate::models::Room,
    email_inbox_id: i64,
    inbox_name: &str,
    new_id: i64,
    body: &str,
) -> Result<(), AppError> {
    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal(
            "freshly inserted email-inbox message vanished".into(),
        ))?;
    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: String::new(),
        author_name: inbox_name.to_string(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: None,
        email_inbox_id: Some(email_inbox_id),
        bridge_id: None,
        bridge_foreign_name: None,
        bridge_kind: None,
        bridge_foreign_avatar: None,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: false,
        client_id: None,
    };
    super::broadcast_room_message(state, room, &event).await?;

    // LC-205: email-ingress posts must reach LC-75 outgoing-webhook
    // subscribers too (bridge daemons, etc.). Mirrors
    // finalize_webhook_message_send; routes through the email_inbox arm of
    // outgoing_actor. Before LC-205 this enqueue was missing, so every
    // outgoing-webhook subscriber silently never saw email-ingress messages.
    crate::outgoing::enqueue(
        &state.chat,
        "message.posted",
        room.id,
        serde_json::json!({
            "message_id": new_id,
            "email_inbox_id": email_inbox_id,
            "author": inbox_name,
            "body": body,
            "actor": super::outgoing_actor("", None, Some(email_inbox_id), None, None),
        }),
    )
    .await;

    // Mentions: an email-ingress sender can @mention people. No author to
    // exclude (empty author id excludes nobody). DM rooms are not a target
    // for email ingress (an inbox is per-room, and DMs have no per-room
    // admin to issue an address), so we gate the mention reconcile by
    // room_type the same way the webhook path does.
    if room.room_type != "dm" {
        let targets = resolve_targets_for_body(state, room, "", body).await?;
        if !targets.is_empty() {
            let (added, _removed) =
                db::mentions::reconcile_mentions(&state.chat, new_id, room.id, "", &targets)
                    .await?;
            let snippet = build_snippet(body);
            let events: Vec<(String, ChatEvent)> = added
                .iter()
                .map(|t| {
                    (
                        t.user_id.clone(),
                        ChatEvent::Mentioned {
                            kind: "mention".into(),
                            room_id: room.id,
                            room_type: room.room_type.clone(),
                            room_label: format!("#{}", room.name),
                            message_id: new_id,
                            mentioned_user_id: t.user_id.clone(),
                            author_label: inbox_name.to_string(),
                            snippet: snippet.clone(),
                            target_path: format!("/room/{}", room.id),
                        },
                    )
                })
                .collect();
            // WS fan-out + LC-77-REPLY email dispatch for each mentioned
            // user. Email-inbox messages don't go through `fanout_mention_events`
            // because they predate the helper (LC-77 v1 inlined the loop);
            // mirror the helper's email-side spawn here.
            for (user_id, event) in events {
                state.hub.broadcast_to_user(&user_id, &event);
                if let ChatEvent::Mentioned { message_id, .. } = &event {
                    let message_id = *message_id;
                    let send_state = state.clone();
                    let room_id = room.id;
                    let room_name = room.name.clone();
                    let user_id_for_dispatch = user_id.clone();
                    tokio::spawn(async move {
                        let _ = crate::email::notification::dispatch_mention_notification(
                            &send_state,
                            &user_id_for_dispatch,
                            message_id,
                            crate::email::notification::NotificationKind::Mention,
                            room_id,
                            &room_name,
                        )
                        .await;
                    });
                }
            }
        }
    }
    Ok(())
}

/// LC-74: broadcast + fan out a message authored by an incoming webhook.
/// Mirrors the non-DM path of `finalize_message_send` but attributes the
/// message to the webhook (empty user_id, `webhook_id` set, name as the
/// author label) and has no author to exclude from mentions.
pub(crate) async fn finalize_webhook_message_send(
    state: &AppState,
    room: &crate::models::Room,
    webhook_id: i64,
    webhook_name: &str,
    new_id: i64,
    body: &str,
) -> Result<(), AppError> {
    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal(
            "freshly inserted webhook message vanished".into(),
        ))?;
    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: String::new(),
        author_name: webhook_name.to_string(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: Some(webhook_id),
        email_inbox_id: None,
        bridge_id: None,
        bridge_foreign_name: None,
        bridge_kind: None,
        bridge_foreign_avatar: None,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: false,
        client_id: None,
    };
    super::broadcast_room_message(state, room, &event).await?;

    // LC-75: outgoing webhooks see incoming-webhook posts too.
    // LC-78: include the `actor` block (kind: "webhook", webhook_id) for
    // daemon self-filter symmetry.
    crate::outgoing::enqueue(
        &state.chat,
        "message.posted",
        room.id,
        serde_json::json!({
            "message_id": new_id,
            "webhook_id": webhook_id,
            "author": webhook_name,
            "body": body,
            "actor": super::outgoing_actor("", Some(webhook_id), None, None, None),
        }),
    )
    .await;

    // Mentions: a webhook can @mention people. No author to exclude (empty
    // author id excludes nobody).
    if room.room_type != "dm" {
        let targets = resolve_targets_for_body(state, room, "", body).await?;
        if !targets.is_empty() {
            let (added, _removed) =
                db::mentions::reconcile_mentions(&state.chat, new_id, room.id, "", &targets)
                    .await?;
            let snippet = build_snippet(body);
            let events: Vec<(String, ChatEvent)> = added
                .iter()
                .map(|t| {
                    (
                        t.user_id.clone(),
                        ChatEvent::Mentioned {
                            kind: "mention".into(),
                            room_id: room.id,
                            room_type: room.room_type.clone(),
                            room_label: format!("#{}", room.name),
                            message_id: new_id,
                            mentioned_user_id: t.user_id.clone(),
                            author_label: webhook_name.to_string(),
                            snippet: snippet.clone(),
                            target_path: format!("/room/{}", room.id),
                        },
                    )
                })
                .collect();
            fanout_mention_events(state, room, events).await;
        }
    }
    Ok(())
}

/// LC-78: broadcast + fan out a message authored by a protocol bridge.
/// Mirrors `finalize_webhook_message_send` but the synthetic-actor identity
/// is per-MESSAGE (snapshotted from the endpoint POST body onto the row),
/// not per-channel. The bridge's room access has already been verified by
/// the caller. `foreign_name` is the snapshotted actor label used as the
/// rendered author + the mention author label.
pub(crate) async fn finalize_bridge_message_send(
    state: &AppState,
    room: &crate::models::Room,
    bridge_id: i64,
    foreign_name: &str,
    kind: &str,
    new_id: i64,
    body: &str,
) -> Result<(), AppError> {
    let raw = db::chat::get_message(&state.chat, new_id)
        .await?
        .ok_or(AppError::Internal(
            "freshly inserted bridge message vanished".into(),
        ))?;
    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: String::new(),
        author_name: foreign_name.to_string(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: None,
        email_inbox_id: None,
        bridge_id: Some(bridge_id),
        bridge_foreign_name: Some(foreign_name.to_string()),
        bridge_kind: Some(kind.to_string()),
        // Snapshot read back from the row the endpoint inserted: the
        // endpoint computed the hash + upserted the proxy row + bound the
        // hash on `insert_bridge_message`, so by the time finalize runs the
        // hash is on the row. None when the daemon submitted no avatar.
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: false,
        client_id: None,
    };
    super::broadcast_room_message(state, room, &event).await?;

    // LC-75: outgoing-webhook fan-out. The actor block lets a daemon
    // self-filter its own bridge's traffic (LC-78 loop-break). bridge_id
    // is the persistent bridges.id, stable across daemon restart, so a
    // daemon that filters on it does not reopen the loop on restart.
    crate::outgoing::enqueue(
        &state.chat,
        "message.posted",
        room.id,
        serde_json::json!({
            "message_id": new_id,
            "bridge_id": bridge_id,
            "author": foreign_name,
            "body": body,
            "actor": super::outgoing_actor("", None, None, Some(bridge_id), Some(foreign_name)),
        }),
    )
    .await;

    // Mentions: a bridge message can @mention people, same as a webhook.
    if room.room_type != "dm" {
        let targets = resolve_targets_for_body(state, room, "", body).await?;
        if !targets.is_empty() {
            let (added, _removed) =
                db::mentions::reconcile_mentions(&state.chat, new_id, room.id, "", &targets)
                    .await?;
            let snippet = build_snippet(body);
            let events: Vec<(String, ChatEvent)> = added
                .iter()
                .map(|t| {
                    (
                        t.user_id.clone(),
                        ChatEvent::Mentioned {
                            kind: "mention".into(),
                            room_id: room.id,
                            room_type: room.room_type.clone(),
                            room_label: format!("#{}", room.name),
                            message_id: new_id,
                            mentioned_user_id: t.user_id.clone(),
                            author_label: foreign_name.to_string(),
                            snippet: snippet.clone(),
                            target_path: format!("/room/{}", room.id),
                        },
                    )
                })
                .collect();
            fanout_mention_events(state, room, events).await;
        }
    }
    Ok(())
}

/// User IDs the caller can mention in this room. For private rooms / DMs,
/// the room's explicit members. For public rooms in an enclave, the
/// enclave's members. Mirrors `routes::mentions::candidate_ids` so the
/// autocomplete and the actual insert agree.
async fn candidate_ids_for_room(
    state: &AppState,
    room: &crate::models::Room,
) -> Result<Vec<String>, AppError> {
    if room.room_type == "private" || room.room_type == "dm" {
        return Ok(db::chat::list_room_member_ids(&state.chat, room.id).await?);
    }
    if let Some(enclave_id) = super::enclave_for_room(state, room.id).await? {
        let members = db::enclave::list_members(&state.chat, enclave_id).await?;
        return Ok(members.into_iter().map(|m| m.user_id).collect());
    }
    Ok(db::auth::list_user_ids(&state.auth).await?)
}

/// Resolve `@channel` against `room`: every candidate member except the
/// author. One bulk auth lookup for usernames.
pub(crate) async fn resolve_channel_targets(
    state: &AppState,
    room: &crate::models::Room,
    author_id: &str,
) -> Result<Vec<db::mentions::MentionRef>, AppError> {
    let candidates = candidate_ids_for_room(state, room).await?;
    let filtered: Vec<String> = candidates
        .into_iter()
        .filter(|id| id != author_id)
        .collect();
    if filtered.is_empty() {
        return Ok(Vec::new());
    }
    let id_refs: Vec<&str> = filtered.iter().map(String::as_str).collect();
    let names = db::auth::display_names_for_ids(&state.auth, &id_refs).await?;
    Ok(filtered
        .into_iter()
        .filter_map(|id| {
            names
                .get(&id)
                .map(|(uname, _dname)| db::mentions::MentionRef {
                    user_id: id,
                    username: uname.clone(),
                })
        })
        .collect())
}

/// Fan out per-user mention notifications. WS broadcasts fire inline
/// (local mpsc, microseconds at any scale). Push dispatches are spawned
/// fire-and-forget so the HTTP handler returns immediately; the actual
/// network-call concurrency cap lives inside `push::dispatch` at the per-
/// subscription send boundary (see `crate::push::PUSH_FANOUT_CONCURRENCY`).
///
/// That layering matters: capping at this outer level would force
/// per-user dispatches to serialize their internal sub-fan-out, regressing
/// the common single-recipient case (a `@username` to a user with 3
/// devices would block the HTTP response for 3 sequential push round-
/// trips). Capping at the inner level binds the only metric that matters
/// (concurrent HTTP to push services) without that latency cost.
async fn fanout_mention_events(
    state: &AppState,
    room: &crate::models::Room,
    events: Vec<(String, ChatEvent)>,
) {
    for (user_id, event) in &events {
        state.hub.broadcast_to_user(user_id, event);
    }
    for (user_id, event) in events {
        let send_state = state.clone();
        let send_state_email = state.clone();
        let room_id = room.id;
        let room_name = room.name.clone();
        // Push dispatch (Web Push / APNs / FCM) - existing path.
        let event_for_push = event.clone();
        let user_for_push = user_id.clone();
        tokio::spawn(async move {
            crate::push::dispatch(&send_state, &user_for_push, &event_for_push).await;
        });
        // LC-77-REPLY: email-side dispatch in parallel. Per-recipient
        // gates + rate limit are in the dispatcher; no need to re-check
        // here. Mention events carry message_id; DM events do too. Both
        // map to NotificationKind::Mention here (the in-app Mentioned
        // event with kind="dm" is only fired from the DM branch which
        // calls dispatch_mention_notification directly with Dm).
        if let ChatEvent::Mentioned { message_id, .. } = &event {
            let message_id = *message_id;
            tokio::spawn(async move {
                let _ = crate::email::notification::dispatch_mention_notification(
                    &send_state_email,
                    &user_id,
                    message_id,
                    crate::email::notification::NotificationKind::Mention,
                    room_id,
                    &room_name,
                )
                .await;
            });
        }
    }
}

/// Walk a parsed token list and return a deduped `Vec<MentionRef>` for
/// `room`. Branches on each token: `@here` and `@channel` (case-
/// insensitive) go through the broadcast resolvers; every other token is
/// treated as a `@username` and looked up via the auth pool. Self-mentions
/// and candidates outside the room's accessibility set are dropped. Final
/// dedup by user_id ensures a user matched by both `@here` and `@username`
/// writes one row, not two.
///
/// Caller is responsible for the DM gate: this helper is non-DM only.
async fn resolve_tokens_for_room(
    state: &AppState,
    room: &crate::models::Room,
    author_id: &str,
    tokens: &[String],
) -> Result<Vec<db::mentions::MentionRef>, AppError> {
    let mut here_seen = false;
    let mut channel_seen = false;
    let mut user_tokens: Vec<&str> = Vec::new();
    for t in tokens {
        match t.to_ascii_lowercase().as_str() {
            "here" => here_seen = true,
            "channel" => channel_seen = true,
            _ => user_tokens.push(t.as_str()),
        }
    }

    let mut targets: Vec<db::mentions::MentionRef> = Vec::new();
    if here_seen {
        targets.extend(resolve_here_targets(state, room, author_id).await?);
    }
    if channel_seen {
        targets.extend(resolve_channel_targets(state, room, author_id).await?);
    }
    if !user_tokens.is_empty() {
        let candidates = candidate_ids_for_room(state, room).await?;
        let candidate_set: std::collections::HashSet<&str> =
            candidates.iter().map(String::as_str).collect();
        // LC-83: a token can resolve to either a user or a per-enclave
        // group. User takes precedence so a group named the same as a
        // username doesn't shadow the user (deletable group; user is
        // canonical). Group expansion drops into the same dedup that
        // collapses @here / @channel / @username overlaps below.
        let enclave_id = super::enclave_for_room(state, room.id).await?;
        for token in user_tokens {
            if let Some(rec) = db::auth::find_user_by_username(&state.auth, token).await? {
                if rec.id == author_id {
                    continue;
                }
                if !candidate_set.contains(rec.id.as_str()) {
                    continue;
                }
                targets.push(db::mentions::MentionRef {
                    user_id: rec.id,
                    username: rec.username,
                });
                continue;
            }
            if let Some(eid) = enclave_id {
                if let Some(group_id) =
                    db::user_groups::find_by_name(&state.chat, eid, token).await?
                {
                    let member_ids =
                        db::user_groups::list_member_ids(&state.chat, group_id).await?;
                    for uid in member_ids {
                        if uid == author_id {
                            continue;
                        }
                        if !candidate_set.contains(uid.as_str()) {
                            continue;
                        }
                        if let Some(rec) = db::auth::find_user_by_id(&state.auth, &uid).await? {
                            targets.push(db::mentions::MentionRef {
                                user_id: rec.id,
                                username: rec.username,
                            });
                        }
                    }
                }
            }
        }
    }

    // Dedup by user_id, preserving first-occurrence order. A user matched
    // by both @here and @username (or by @here and @channel) should get one
    // row, not two.
    let mut seen = std::collections::HashSet::new();
    targets.retain(|m| seen.insert(m.user_id.clone()));
    Ok(targets)
}

/// LC-304: resolve everyone who should get a mention row for `body` in `room` -
/// the explicit `@token` targets PLUS any member whose configured highlight
/// word appears in the body. Deduped. All message-creation reconcile sites
/// (post / edit / webhook / email-ingress / bridge) call this so keyword
/// matches behave exactly like @mentions (badge, row highlight, inbox, notify).
async fn resolve_targets_for_body(
    state: &AppState,
    room: &crate::models::Room,
    author_id: &str,
    body: &str,
) -> Result<Vec<db::mentions::MentionRef>, AppError> {
    let tokens = db::mentions::parse_mention_tokens(body);
    let mut targets = resolve_tokens_for_room(state, room, author_id, &tokens).await?;
    append_keyword_targets(state, room, author_id, body, &mut targets).await?;
    Ok(targets)
}

/// Append members whose LC-304 highlight word matches `body` to `targets`,
/// skipping the author, non-members, and anyone already targeted by an
/// @mention. Cheap when nothing matches: the room's member set is resolved
/// only after a configured word actually hits the body.
async fn append_keyword_targets(
    state: &AppState,
    room: &crate::models::Room,
    author_id: &str,
    body: &str,
    targets: &mut Vec<db::mentions::MentionRef>,
) -> Result<(), AppError> {
    let keywords = db::notification_keywords::all(&state.auth).await?;
    if keywords.is_empty() {
        return Ok(());
    }
    let body_lower = body.to_lowercase();
    let already: std::collections::HashSet<&str> =
        targets.iter().map(|m| m.user_id.as_str()).collect();
    let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (uid, word) in &keywords {
        if uid == author_id || already.contains(uid.as_str()) || matched.contains(uid) {
            continue;
        }
        if keyword_matches_body(&body_lower, &word.to_lowercase()) {
            matched.insert(uid.clone());
        }
    }
    if matched.is_empty() {
        return Ok(());
    }
    // Only room members may be keyword-targeted (a mention row for a non-member
    // would surface a message they cannot open). resolve_channel_targets yields
    // exactly the member set minus the author, already carrying usernames.
    let members = resolve_channel_targets(state, room, author_id).await?;
    let mut seen: std::collections::HashSet<String> =
        already.iter().map(|s| s.to_string()).collect();
    for m in members {
        if matched.contains(&m.user_id) && seen.insert(m.user_id.clone()) {
            targets.push(m);
        }
    }
    Ok(())
}

/// Case-insensitive, word-boundary keyword match; both inputs must already be
/// lowercased. "deploy" matches "deploy" / "deploy!" / "the deploy" but not
/// "deployment"; a multi-word phrase matches as a bounded substring.
fn keyword_matches_body(body_lower: &str, word_lower: &str) -> bool {
    if word_lower.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = body_lower[from..].find(word_lower) {
        let start = from + rel;
        let end = start + word_lower.len();
        let before_ok = body_lower[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = body_lower[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = end;
        if from >= body_lower.len() {
            break;
        }
    }
    false
}

/// Resolve `@here` against `room`: every candidate member who has at least
/// one live WebSocket connection AND whose persisted status is neither DND
/// nor manual `away`. Excludes the author. Auto-`idle` is intentionally
/// included: idle is "stepped away briefly," whereas manual `away` is the
/// user explicitly saying "I'm not around right now."
pub(crate) async fn resolve_here_targets(
    state: &AppState,
    room: &crate::models::Room,
    author_id: &str,
) -> Result<Vec<db::mentions::MentionRef>, AppError> {
    let candidates = candidate_ids_for_room(state, room).await?;
    // First filter: cheap, in-memory hub check + author exclusion.
    let connected: Vec<String> = candidates
        .into_iter()
        .filter(|id| id != author_id && state.hub.is_user_connected(id))
        .collect();
    if connected.is_empty() {
        return Ok(Vec::new());
    }
    // Second filter: bulk auth lookup to drop DND/away users and resolve usernames.
    let id_refs: Vec<&str> = connected.iter().map(String::as_str).collect();
    let rows = db::auth::usernames_and_status_for_ids(&state.auth, &id_refs).await?;
    Ok(connected
        .into_iter()
        .filter_map(|id| {
            rows.get(&id).and_then(|(uname, status)| {
                if status == db::auth::STATUS_DND || status == db::auth::STATUS_AWAY {
                    None
                } else {
                    Some(db::mentions::MentionRef {
                        user_id: id,
                        username: uname.clone(),
                    })
                }
            })
        })
        .collect())
}

/// `display_name` if set, else `username`. Used for `Mentioned.author_label`.
fn author_label(user: &User) -> String {
    user.display_name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(&user.username)
        .to_string()
}

/// Build a short plain-text preview of the message body for the
/// notification surface. Strips leading `@` chars from mention tokens so
/// the snippet reads naturally; truncates at 140 chars with an ellipsis.
fn build_snippet(body: &str) -> String {
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let max = 140;
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(max).collect();
        format!("{cut}...")
    }
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
    if db::auth::is_blocked_either_way(&state.auth, &user.id, &m.user_id).await? {
        return Err(AppError::NotFound);
    }
    let pinned_ids = db::pinned::pinned_message_ids_for_room(&state.chat, m.room_id).await?;
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &user.id, message_id).await?;
    let view = super::load_message_view_for_viewer(
        &state,
        &user,
        message_id,
        pinned_ids.contains(&message_id),
        is_bookmarked,
    )
    .await?;
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
    check_message_length(body)?;

    let edited_at_str = db::chat::update_message_body(&state.chat, message_id, body).await?;

    let event = ChatEvent::MessageEdited {
        message_id,
        room_id: m.room_id,
        new_body: body.to_string(),
        edited_at: edited_at_str.clone(),
    };
    state.hub.broadcast_to_room(m.room_id, &event);

    // LC-75: outgoing webhooks.
    // LC-78: actor describes the message AUTHOR (not the editor) so a bridge
    // daemon can self-filter edits to messages it produced.
    crate::outgoing::enqueue(
        &state.chat,
        "message.edited",
        m.room_id,
        serde_json::json!({
            "message_id": message_id,
            "body": body,
            "actor": super::outgoing_actor(
                &m.user_id,
                m.webhook_id,
                m.email_inbox_id,
                m.bridge_id,
                m.bridge_foreign_name.as_deref(),
            ),
        }),
    )
    .await;

    // Reconcile mention rows. Skipped for DM rooms (no rows to reconcile).
    // Token resolution goes through the same helper as post_message so an
    // edit that adds/removes `@here` / `@channel` / `@username` diffs through
    // reconcile_mentions correctly (rows for users in both sets keep their
    // read_at; rows newly resolved get inserted; rows no longer resolved get
    // deleted with a MentionCleared event).
    let edited_room = db::chat::get_room(&state.chat, m.room_id).await?;
    if let Some(ref edited_room) = edited_room {
        if edited_room.room_type != "dm" {
            let targets = resolve_targets_for_body(&state, edited_room, &user.id, body).await?;
            let (added, removed) = db::mentions::reconcile_mentions(
                &state.chat,
                message_id,
                m.room_id,
                &user.id,
                &targets,
            )
            .await?;
            let snippet = build_snippet(body);
            let author_label = author_label(&user);
            let events: Vec<(String, ChatEvent)> = added
                .iter()
                .map(|t| {
                    (
                        t.user_id.clone(),
                        ChatEvent::Mentioned {
                            kind: "mention".into(),
                            room_id: m.room_id,
                            room_type: edited_room.room_type.clone(),
                            room_label: format!("#{}", edited_room.name),
                            message_id,
                            mentioned_user_id: t.user_id.clone(),
                            author_label: author_label.clone(),
                            snippet: snippet.clone(),
                            target_path: format!("/room/{}", m.room_id),
                        },
                    )
                })
                .collect();
            fanout_mention_events(&state, edited_room, events).await;
            // MentionCleared events are WS-only (no Push, no badge attention)
            // and cheap to fire inline. Keep them sequential.
            for t in &removed {
                let event = ChatEvent::MentionCleared {
                    room_id: m.room_id,
                    mentioned_user_id: t.user_id.clone(),
                    message_id,
                };
                state.hub.broadcast_to_user(&t.user_id, &event);
            }
        }
    }

    // Render the updated message as a single-message fragment so the sender's
    // edit form is replaced inline.
    let meta = super::resolve_msg_author(
        &state,
        &m.user_id,
        m.webhook_id,
        m.email_inbox_id,
        m.bridge_id,
        m.bridge_foreign_name.as_deref(),
        m.bridge_kind.as_deref(),
        m.bridge_foreign_avatar.as_deref(),
        &user.id,
    )
    .await?;
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
    let reaction_counts = db::chat::list_reactions(&state.chat, m.id, &user.id).await?;
    let reactor_titles = super::build_reactor_titles(&state, &reaction_counts).await;
    let reactions: Vec<ReactionView> = reaction_counts
        .into_iter()
        .zip(reactor_titles)
        .map(|(r, title)| {
            ReactionView::new(r.emoji, r.count, r.reacted_by_me, title, &custom_emojis)
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
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
        .await?
        .remove(&m.id)
        .unwrap_or_default();
    let quote_preview = match m.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid).await?,
        None => None,
    };
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
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        reply_count,
        parent_id: m.parent_id,
        attachments,
        mentions,
        is_pinned: false,
        is_bookmarked: db::bookmarks::is_bookmarked(&state.chat, &user.id, m.id).await?,
        custom_emojis,
        quote_preview,
        is_system: m.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, m.id, &user.id)
            .await
            .ok()
            .flatten(),
        author_is_bot: meta.is_bot,
        actor: meta.actor.clone(),
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

    let blocked_authors = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    if blocked_authors.contains(&parent.user_id) {
        return Err(AppError::NotFound);
    }
    let raw_replies: Vec<_> = db::chat::list_thread_replies(&state.chat, message_id)
        .await?
        .into_iter()
        .filter(|r| !blocked_authors.contains(&r.user_id))
        .collect();
    let mut author_cache: HashMap<String, super::AuthorMeta> = HashMap::new();
    let parent_meta = super::resolve_msg_author(
        &state,
        &parent.user_id,
        parent.webhook_id,
        parent.email_inbox_id,
        parent.bridge_id,
        parent.bridge_foreign_name.as_deref(),
        parent.bridge_kind.as_deref(),
        parent.bridge_foreign_avatar.as_deref(),
        &user.id,
    )
    .await?;

    // Bulk-load attachments for the parent and every reply in a single query.
    let mut all_ids: Vec<i64> = raw_replies.iter().map(|r| r.id).collect();
    all_ids.push(parent.id);
    let mut attachments_by_message =
        db::uploads::attachments_for_messages(&state.chat, &all_ids).await?;
    let mut mentions_by_message =
        db::mentions::mentions_for_messages(&state.chat, &state.auth, &all_ids).await?;
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, room_id).await?;
    let bookmarked_ids =
        db::bookmarks::bookmarked_message_ids_in_room(&state.chat, &user.id, room_id).await?;
    // LC-66: polls among the parent + replies, so the loop builds poll views
    // only for actual polls.
    let poll_ids = db::polls::poll_message_ids(&state.chat, &all_ids).await?;

    let parent_quote_preview = match parent.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid).await?,
        None => None,
    };
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
        // LC-244: per-message render; the client inserts live day dividers.
        day_label: None,
        reply_count: 0,
        parent_id: None,
        attachments: attachments_by_message
            .remove(&parent.id)
            .unwrap_or_default(),
        mentions: mentions_by_message.remove(&parent.id).unwrap_or_default(),
        is_pinned: false,
        is_bookmarked: bookmarked_ids.contains(&parent.id),
        custom_emojis: custom_emojis.clone(),
        quote_preview: parent_quote_preview,
        is_system: parent.is_system,
        poll: if poll_ids.contains(&parent.id) {
            crate::views::room::build_poll_view(&state.chat, &state.auth, parent.id, &user.id)
                .await
                .ok()
                .flatten()
        } else {
            None
        },
        author_is_bot: parent_meta.is_bot,
        actor: parent_meta.actor.clone(),
    };

    let mut replies: Vec<MessageView> = Vec::with_capacity(raw_replies.len());
    for r in raw_replies {
        let meta = if let Some(entry) = author_cache.get(&r.user_id) {
            entry.clone()
        } else {
            let entry = super::resolve_msg_author(
                &state,
                &r.user_id,
                r.webhook_id,
                r.email_inbox_id,
                r.bridge_id,
                r.bridge_foreign_name.as_deref(),
                r.bridge_kind.as_deref(),
                r.bridge_foreign_avatar.as_deref(),
                &user.id,
            )
            .await?;
            author_cache.insert(r.user_id.clone(), entry.clone());
            entry
        };
        let attachments = attachments_by_message.remove(&r.id).unwrap_or_default();
        let mentions = mentions_by_message.remove(&r.id).unwrap_or_default();
        let r_id = r.id;
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
            // LC-244: per-message render; the client inserts live day dividers.
            day_label: None,
            reply_count: 0,
            parent_id: r.parent_id,
            attachments,
            mentions,
            is_pinned: false,
            is_bookmarked: bookmarked_ids.contains(&r_id),
            custom_emojis: custom_emojis.clone(),
            quote_preview: None,
            is_system: r.is_system,
            poll: if poll_ids.contains(&r_id) {
                crate::views::room::build_poll_view(&state.chat, &state.auth, r_id, &user.id)
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

    // LC-310: whether the viewer follows this thread (drives the toggle).
    let is_following =
        db::thread_followers::is_following(&state.chat, &user.id, message_id).await?;

    let fragment = ThreadPanelFragment {
        room: &room,
        parent: &parent_view,
        replies: &replies,
        is_following,
    };
    html(&fragment)
}

/// POST /room/{room_id}/thread/{parent_id}/follow - subscribe to a thread's
/// replies. DELETE unsubscribes. Both re-render the toggle button. Room-access
/// + parent gated like the reply handler.
pub async fn post_thread_follow(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, parent_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    thread_follow_toggle(&state, &user, room_id, parent_id, true).await
}

pub async fn delete_thread_follow(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, parent_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    thread_follow_toggle(&state, &user, room_id, parent_id, false).await
}

async fn thread_follow_toggle(
    state: &AppState,
    user: &User,
    room_id: i64,
    parent_id: i64,
    follow: bool,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let parent = db::chat::get_message(&state.chat, parent_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if parent.room_id != room_id || parent.parent_id.is_some() {
        return Err(AppError::NotFound);
    }
    if follow {
        db::thread_followers::follow(&state.chat, &user.id, parent_id, room_id).await?;
    } else {
        db::thread_followers::unfollow(&state.chat, &user.id, parent_id).await?;
    }
    html(&crate::views::room::ThreadFollowFragment {
        room_id,
        parent_id,
        following: follow,
    })
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
    check_message_length(body)?;
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
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };
    let event = ChatEvent::ThreadReply { parent_id, message };
    state.hub.stop_thread_typing(room_id, parent_id, &user.id);
    super::broadcast_room_message(&state, &room, &event).await?;

    // LC-310: thread following. Auto-follow the replier and the thread root's
    // author (so they hear about replies to their message), then notify every
    // follower of this new reply. Follows are NOT mention rows; we reuse only
    // the mention fan-out (WS toast + push + email, mute-gated). Best-effort:
    // a follow/notify hiccup must not fail the reply that already landed.
    let _ = db::thread_followers::follow(&state.chat, &user.id, parent_id, room_id).await;
    if parent.user_id != user.id {
        let _ =
            db::thread_followers::follow(&state.chat, &parent.user_id, parent_id, room_id).await;
    }
    notify_thread_followers(&state, &room, parent_id, new_id, &user, body).await;

    // Empty 204 - composer clears via hx-on::before-request, no body needed.
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// LC-310: fan out a `kind="thread"` notification to every follower of the
/// thread rooted at `parent_id` for the just-posted reply `reply_id`, excluding
/// the reply's author and any follower who no longer has access to the room
/// (so a departed member is never pinged). Reuses `fanout_mention_events`, so
/// the WS toast + push + email all honor the recipient's prefs and room mute.
async fn notify_thread_followers(
    state: &AppState,
    room: &crate::models::Room,
    parent_id: i64,
    reply_id: i64,
    author: &User,
    body: &str,
) {
    let followers = match db::thread_followers::followers(&state.chat, parent_id).await {
        Ok(f) => f,
        Err(_) => return,
    };
    let snippet = build_snippet(body);
    let author_label = author_label(author);
    let mut events: Vec<(String, ChatEvent)> = Vec::new();
    for uid in followers {
        if uid == author.id {
            continue;
        }
        // The follower must still be able to see the room. is_room_accessible
        // is the same gate mentions use; the follower count is small.
        if !db::chat::is_room_accessible(&state.chat, room.id, &uid, false)
            .await
            .unwrap_or(false)
        {
            continue;
        }
        events.push((
            uid.clone(),
            ChatEvent::Mentioned {
                kind: "thread".into(),
                room_id: room.id,
                room_type: room.room_type.clone(),
                room_label: format!("#{}", room.name),
                message_id: reply_id,
                mentioned_user_id: uid,
                author_label: author_label.clone(),
                snippet: snippet.clone(),
                target_path: format!("/room/{}", room.id),
            },
        ));
    }
    if !events.is_empty() {
        fanout_mention_events(state, room, events).await;
    }
}

/// DELETE /thread-panel - close the panel by replacing it with an empty
/// hidden aside.
pub async fn close_thread_panel() -> Result<Html, AppError> {
    html(&ThreadPanelClosedFragment)
}

/// GET /messages/:id/history - render the edit-history drawer for a
/// message. Permission gate combines `get_message` (rejects soft-deleted)
/// and `is_room_accessible` (room membership), matching the access shape of
/// `get_single_message` exactly. Prior bodies render with mention chips
/// resolved through `db::mentions::mentions_for_body` so chips persist for
/// users a later edit removed; the current body uses the live-path
/// `mentions_for_messages` for consistency with the room timeline.
pub async fn get_history_panel(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, m.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let edits = db::chat::list_message_edits(&state.chat, message_id).await?;
    let emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
    let current_mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
        .await?
        .remove(&m.id)
        .unwrap_or_default();
    let mut entries: Vec<HistoryEntryView> = Vec::with_capacity(edits.len() + 1);
    for e in edits {
        let prior_mentions = db::mentions::mentions_for_body(&state.auth, &e.previous_body).await?;
        entries.push(HistoryEntryView {
            body_html: crate::views::markdown::render(&e.previous_body, &prior_mentions, &emojis),
            label: format!("Edited {}", e.edited_at),
        });
    }
    let current_label = match m.edited_at.as_deref() {
        Some(ts) => format!("Current - last edited {ts}"),
        None => "Current".to_string(),
    };
    entries.push(HistoryEntryView {
        body_html: crate::views::markdown::render(&m.body, &current_mentions, &emojis),
        label: current_label,
    });
    html(&HistoryPanelFragment {
        message_id,
        entries: &entries,
    })
}

/// DELETE /history-panel - close the drawer by replacing it with an empty
/// hidden aside.
pub async fn close_history_panel() -> Result<Html, AppError> {
    html(&HistoryPanelClosedFragment)
}

/// GET /m/:message_id - LC-246 message permalink. Resolves the message's
/// canonical page and 302-redirects there with a `#msg-{id}` fragment so the
/// client (`partials/auto_scroll.html`) scrolls to and highlights it:
/// `/dm/{peer}#msg-{id}` for a DM, `/room/{room_id}#msg-{id}` otherwise.
///
/// Returns 404 (not 403) for a missing, deleted, quarantined, or
/// inaccessible message: the permalink must never reveal that a message
/// exists in a room the viewer cannot see. Thread replies redirect to the
/// room (the reply lives in the thread panel, not the main list), where the
/// highlight no-ops gracefully.
pub async fn get_message_permalink(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Redirect, AppError> {
    let Some(msg) = db::chat::get_message(&state.chat, message_id).await? else {
        return Err(AppError::NotFound);
    };
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, msg.room_id, &user.id, is_admin).await? {
        // 404, not 403: do not confirm the message exists in a hidden room.
        return Err(AppError::NotFound);
    }
    let Some(room) = db::chat::get_room(&state.chat, msg.room_id).await? else {
        return Err(AppError::NotFound);
    };
    let target = if room.room_type == "dm" {
        match db::chat::get_dm_peer(&state.chat, msg.room_id, &user.id).await? {
            Some(peer_id) => format!("/dm/{peer_id}#msg-{message_id}"),
            // The viewer is not a participant in this DM; treat as not found.
            None => return Err(AppError::NotFound),
        }
    } else {
        format!("/room/{}#msg-{message_id}", msg.room_id)
    };
    Ok(Redirect::to(&target))
}

/// GET /room/:room_id/composer-quote/:message_id - returns the inline
/// "Replying to ..." chip that fills `#composer-quote-bar` inside the
/// composer. The chip carries a hidden `quote_id` input so the next submit
/// picks it up along with the body. Refuses when the target message is in
/// a different room, soft-deleted, or itself a thread reply (quote-replies
/// only apply to top-level messages).
pub async fn get_composer_quote(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, message_id)): Path<(i64, i64)>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let m = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if m.room_id != room_id || m.parent_id.is_some() {
        return Err(AppError::NotFound);
    }
    let preview = crate::views::room::build_quote_preview(&state.chat, &state.auth, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    html(&ComposerQuoteChip {
        quote_id: preview.id,
        author_label: preview.author_label,
        body_excerpt: preview.body_excerpt,
    })
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
    let can_delete = m.user_id == user.id
        || db::room_rbac::is_room_moderator(&state.chat, m.room_id, &user.id, &user.role).await?;
    if !can_delete {
        return Err(AppError::Forbidden);
    }
    // Look up the next message in the room BEFORE soft-deleting so we can
    // detect whether deleting `m` exposes an orphaned follow-up. Capturing
    // the prior state of `m` here lets the regrouping decision use the
    // pre-delete grouping invariant.
    let next = db::chat::next_message_in_room(&state.chat, m.room_id, message_id).await?;

    // Drop any mention rows tied to this message and tell each mentioned
    // user to decrement their unread mention count. Done BEFORE the
    // soft-delete so the row lookup still sees the unread rows.
    let cleared_for_users =
        db::mentions::delete_mentions_for_message(&state.chat, message_id).await?;
    for uid in &cleared_for_users {
        let event = ChatEvent::MentionCleared {
            room_id: m.room_id,
            mentioned_user_id: uid.clone(),
            message_id,
        };
        state.hub.broadcast_to_user(uid, &event);
    }

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

    // LC-75: outgoing webhooks.
    // LC-78: actor describes the message AUTHOR so a bridge daemon can
    // self-filter deletions of messages it produced.
    crate::outgoing::enqueue(
        &state.chat,
        "message.deleted",
        m.room_id,
        serde_json::json!({
            "message_id": message_id,
            "actor": super::outgoing_actor(
                &m.user_id,
                m.webhook_id,
                m.email_inbox_id,
                m.bridge_id,
                m.bridge_foreign_name.as_deref(),
            ),
        }),
    )
    .await;
    // Return the deleted-fragment HTML directly so the requesting tab also updates.
    let body = format!(
        "<div id=\"msg-{message_id}\" class=\"px-4 py-2 italic text-slate-400\">[deleted]</div>"
    );
    Ok(axum::response::Html(body).into_response())
}

/// LC-85: does the caller satisfy the room's posting policy? `"all"`
/// is always true; `"moderators_only"` requires effective role
/// moderator or admin; `"admins_only"` requires effective role admin.
/// The effective-role check consumes `db::room_rbac` (LC-84) so a
/// per-room moderator override unlocks `moderators_only` rooms even
/// for an org-Member.
pub(crate) async fn can_post_with_policy(
    state: &AppState,
    user: &User,
    room_id: i64,
    policy: &str,
) -> Result<bool, AppError> {
    match policy {
        "all" => Ok(true),
        "moderators_only" => {
            Ok(
                db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role)
                    .await?,
            )
        }
        "admins_only" => {
            let r =
                db::room_rbac::effective_role(&state.chat, room_id, &user.id, &user.role).await?;
            Ok(r == "admin")
        }
        // Unknown policy strings fall through closed. The CHECK constraint
        // should make this unreachable; defending against schema drift.
        _ => Ok(false),
    }
}
