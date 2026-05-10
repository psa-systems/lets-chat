use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use askama::Template;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::auth::OptionalUser;
use crate::db;
use crate::models::{self, User};
use crate::state::AppState;
use crate::views::room::ReplyCountFragment;
use crate::views::room::{MessageView, ReactionView};
use crate::views::ws_fragments::{
    render_event, EditedMessageFragment, MentionClearedFragment, MentionedFragment,
    NewMessageFragment, ReactionUpdateFragment, SeenIndicatorFragment, SidebarUpdateFragment,
    ThreadReplyOobFragment, UnreadBadgeFragment,
};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "subscribe")]
    Subscribe { room_id: i64 },
    #[serde(rename = "typing")]
    Typing { room_id: i64 },
    #[serde(rename = "thread_typing")]
    ThreadTyping { room_id: i64, parent_id: i64 },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
) -> impl IntoResponse {
    let Some(user) = user else {
        return (http::StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: User) {
    let username = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let (conn_id, mut rx, is_first_conn) = state.hub.connect(&user.id, &username);
    if is_first_conn {
        // Re-fetch so the broadcast carries the freshest persisted status,
        // not the snapshot the inject_user middleware took at HTTP time.
        if let Ok(Some(rec)) = db::auth::find_user_by_id(&state.auth, &user.id).await {
            state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
                user_id: rec.id,
                status: rec.status,
                custom_status: rec.custom_status,
            });
        }
    }
    super::touch_user_and_maybe_broadcast(&state, &user.id).await;
    let subscribed: Arc<Mutex<HashSet<i64>>> = Arc::new(Mutex::new(HashSet::new()));
    // Per-connection memory of which own-authored DM message currently shows
    // the "Seen HH:MM" caption, keyed by room_id. Used to clear the previous
    // slot when the peer reads further.
    let dm_seen_msg: Arc<Mutex<HashMap<i64, i64>>> = Arc::new(Mutex::new(HashMap::new()));
    let (mut tx, mut rx_ws) = socket.split();

    let send_state = state.clone();
    let send_user = user.clone();
    let send_subscribed = subscribed.clone();
    let send_dm_seen = dm_seen_msg.clone();
    let send = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.tick().await;
        loop {
            tokio::select! {
                evt = rx.recv() => {
                    match evt {
                        Ok(e) => {
                            let rendered = match &e {
                                ChatEvent::NewMessage { message, .. } => {
                                    render_new_message_or_bump(
                                        &send_state,
                                        message,
                                        &send_user,
                                        &send_subscribed,
                                    )
                                    .await
                                }
                                ChatEvent::MessageEdited { message_id, .. }
                                | ChatEvent::MessageRegrouped { message_id, .. } => {
                                    render_edited_message(&send_state, *message_id, &send_user).await
                                }
                                ChatEvent::ReactionAdded { message_id, .. }
                                | ChatEvent::ReactionRemoved { message_id, .. } => {
                                    render_reaction_bar(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::DmRead { user_id, room_id, last_read_message_id, read_at } => {
                                    render_dm_read(
                                        &send_state,
                                        &send_user,
                                        *room_id,
                                        user_id,
                                        *last_read_message_id,
                                        read_at,
                                        &send_dm_seen,
                                    )
                                    .await
                                }
                                ChatEvent::RoomMemberAdded { user_id, .. }
                                | ChatEvent::RoomMemberRemoved { user_id, .. } => {
                                    if user_id == &send_user.id {
                                        render_sidebar(&send_state, &send_user).await
                                    } else {
                                        None
                                    }
                                }
                                ChatEvent::ThreadReply { parent_id, message } => {
                                    render_thread_reply(
                                        &send_state,
                                        *parent_id,
                                        message,
                                        &send_user,
                                    )
                                    .await
                                }
                                ChatEvent::Mentioned {
                                    mentioned_user_id,
                                    room_id,
                                    ..
                                } if mentioned_user_id == &send_user.id => {
                                    // `MuteMode::All` suppresses the event
                                    // entirely for both room and DM kinds.
                                    // `ExceptMentions` falls through to render;
                                    // it is unreachable for DM rooms via the
                                    // API (`set_dm_mute` only writes None/All)
                                    // but a corrupt row would render rather
                                    // than crash.
                                    let allow = db::notifications::room_mute_mode(
                                        &send_state.chat,
                                        &send_user.id,
                                        *room_id,
                                    )
                                    .await
                                    .unwrap_or(db::notifications::MuteMode::None)
                                    .allows_room_mention();
                                    if allow {
                                        render_mentioned(&e)
                                    } else {
                                        None
                                    }
                                }
                                ChatEvent::MentionCleared {
                                    mentioned_user_id, ..
                                } if mentioned_user_id == &send_user.id => {
                                    render_mention_cleared(&e)
                                }
                                ChatEvent::RoomNotifyPrefsChanged { user_id, .. }
                                    if user_id == &send_user.id =>
                                {
                                    render_sidebar(&send_state, &send_user).await
                                }
                                ChatEvent::DmMuteChanged { .. } => {
                                    // Routed only via
                                    // `Hub::broadcast_to_user(muter_id, ...)`,
                                    // so reaching this arm already implies
                                    // the recipient is the muter. Re-render
                                    // the sidebar OOB so the peer row's
                                    // greyed-link class and unread-badge
                                    // visibility flip in this tab.
                                    render_sidebar(&send_state, &send_user).await
                                }
                                ChatEvent::MessagePinned { room_id, .. }
                                | ChatEvent::MessageUnpinned { room_id, .. } => {
                                    // Pin/unpin events fan out to every
                                    // subscriber of the affected room. We
                                    // rebuild the strip fragment for this
                                    // viewer (so the pinner display label is
                                    // resolved against this user's auth view)
                                    // and emit the OOB swap. The strip is
                                    // room-scoped, not user-scoped, so the
                                    // viewer-id check is unnecessary.
                                    render_pinned_strip(&send_state, &send_user, *room_id).await
                                }
                                _ => render_event(&e),
                            };
                            if let Some(html) = rendered {
                                if tx.send(Message::Text(html.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                _ = ping.tick() => {
                    // Send an HTML-comment text frame as the heartbeat. The
                    // client uses any inbound `htmx:wsAfterMessage` to reset its
                    // half-open watchdog. Comments are not `fragment.children`
                    // for `htmx-ext-ws`, so the swap path renders a no-op.
                    if tx.send(Message::Text("<!-- ping -->".into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    while let Some(Ok(msg)) = rx_ws.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(frame) = serde_json::from_str::<ClientFrame>(text.as_str()) {
                    match frame {
                        ClientFrame::Subscribe { room_id } => {
                            let allowed = match db::chat::get_room(&state.chat, room_id).await {
                                Ok(Some(r)) if r.room_type == "dm" || r.room_type == "private" => {
                                    db::chat::is_room_member(&state.chat, room_id, &user.id)
                                        .await
                                        .unwrap_or(false)
                                }
                                Ok(Some(_)) => true,
                                _ => false,
                            };
                            if allowed {
                                state.hub.subscribe(conn_id, room_id);
                                subscribed.lock().unwrap().insert(room_id);
                            }
                        }
                        ClientFrame::Typing { room_id } => {
                            if subscribed.lock().unwrap().contains(&room_id) {
                                state.hub.notify_typing(conn_id, room_id);
                                super::touch_user_and_maybe_broadcast(&state, &user.id).await;
                            }
                        }
                        ClientFrame::ThreadTyping { room_id, parent_id } => {
                            if subscribed.lock().unwrap().contains(&room_id) {
                                state.hub.notify_thread_typing(conn_id, room_id, parent_id);
                                super::touch_user_and_maybe_broadcast(&state, &user.id).await;
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    if let Some(uid) = state.hub.disconnect(conn_id) {
        state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
            user_id: uid,
            status: "offline".to_string(),
            custom_status: None,
        });
    }
    send.abort();
}

/// Render a single sidebar unread badge OOB swap reflecting the viewer's
/// current unread count for `room_id`. `room` is the resolved Room used to
/// pick the badge id encoding (kind="dm" + peer_id vs kind="room" + room_id).
async fn render_unread_badge(
    state: &AppState,
    viewer: &User,
    room: &models::Room,
) -> Option<String> {
    let unread = db::chat::get_unread_count(&state.chat, &viewer.id, room.id)
        .await
        .ok()?;
    if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room.id, &viewer.id)
            .await
            .ok()??;
        UnreadBadgeFragment {
            kind: "dm",
            id: &peer_id,
            unread,
            mute_mode: "none",
        }
        .render()
        .ok()
    } else {
        let id_str = room.id.to_string();
        UnreadBadgeFragment {
            kind: "room",
            id: &id_str,
            unread,
            mute_mode: "none",
        }
        .render()
        .ok()
    }
}

/// Handle a `DmRead` event. Two distinct sub-cases live in the same arm:
///
/// 1. `actor_user_id == viewer.id` - the viewer themselves opened/refreshed the
///    room in another tab. Re-render their sidebar badge so it clears live
///    across all of their sessions.
///
/// 2. `actor_user_id != viewer.id` - the peer read the viewer's messages in a
///    DM. If both parties have read receipts enabled, render the "Seen HH:MM"
///    caption under the most recent own-authored message <= peer's
///    `last_read_message_id`, and clear the previous caption slot if any.
async fn render_dm_read(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    actor_user_id: &str,
    last_read_message_id: i64,
    read_at: &str,
    dm_seen_msg: &Arc<Mutex<HashMap<i64, i64>>>,
) -> Option<String> {
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;

    if actor_user_id == viewer.id {
        return render_unread_badge(state, viewer, &room).await;
    }

    if room.room_type != "dm" {
        return None;
    }

    if !viewer.read_receipts_enabled {
        return None;
    }

    // Symmetric consent: peer must have receipts enabled for the viewer to
    // see "Seen". Look up the peer's user record from auth.
    let peer_record = db::auth::find_user_by_id(&state.auth, actor_user_id)
        .await
        .ok()??;
    if !peer_record.read_receipts_enabled {
        return None;
    }

    // Find the highest own-authored, non-deleted message in this DM with id
    // <= last_read_message_id. If none, peer hasn't read any of viewer's
    // messages yet - clear any previous caption and stop.
    let new_seen_id = db::chat::find_dm_seen_state(&state.chat, room_id, &viewer.id, actor_user_id)
        .await
        .ok()?
        .map(|(id, _)| id)
        .filter(|id| *id <= last_read_message_id);

    let prev_seen_id = {
        let map = dm_seen_msg.lock().unwrap();
        map.get(&room_id).copied()
    };

    if new_seen_id == prev_seen_id {
        return None;
    }

    let mut html = String::new();
    if let Some(prev) = prev_seen_id {
        if Some(prev) != new_seen_id {
            if let Ok(frag) = (SeenIndicatorFragment {
                message_id: prev,
                caption: None,
            })
            .render()
            {
                html.push_str(&frag);
            }
        }
    }
    if let Some(new_id) = new_seen_id {
        let hhmm = super::dm::format_hhmm(read_at);
        if let Ok(frag) = (SeenIndicatorFragment {
            message_id: new_id,
            caption: Some(&hhmm),
        })
        .render()
        {
            html.push_str(&frag);
        }
    }

    {
        let mut map = dm_seen_msg.lock().unwrap();
        match new_seen_id {
            Some(id) => {
                map.insert(room_id, id);
            }
            None => {
                map.remove(&room_id);
            }
        }
    }

    if html.is_empty() {
        None
    } else {
        Some(html)
    }
}

/// Pick the right rendering for a `NewMessage` event for this connection:
///
/// - If the viewer's connection is currently subscribed to the room (room is
///   open in the foreground), render the message into `#messages`.
/// - Else, if the message was authored by someone other than the viewer,
///   render an unread-badge bump for the sidebar.
/// - Otherwise (own message, no open subscription), render nothing.
async fn render_new_message_or_bump(
    state: &AppState,
    message: &models::Message,
    viewer: &User,
    subscribed: &Arc<Mutex<HashSet<i64>>>,
) -> Option<String> {
    // Suppress live delivery from blocked authors. Skips both the foreground
    // render and the sidebar unread bump so a blocked user cannot poke the
    // viewer's UI.
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &message.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let is_subscribed = subscribed.lock().unwrap().contains(&message.room_id);
    if is_subscribed {
        // The viewer has the room open in the foreground, so the message is
        // effectively read on arrival. Advance their last-read watermark and
        // broadcast a DmRead so the author sees a live "Seen" update (in DMs)
        // and any other tabs of this user clear their sidebar badge. Skip
        // when the viewer authored the message - their own send path already
        // re-marks read state.
        if message.user_id != viewer.id {
            if let Ok(read_at) =
                db::chat::set_last_read(&state.chat, &viewer.id, message.room_id, message.id).await
            {
                db::mentions::mark_mentions_read_for_room(
                    &state.chat,
                    &viewer.id,
                    message.room_id,
                    message.id,
                )
                .await
                .ok();
                let event = ChatEvent::DmRead {
                    room_id: message.room_id,
                    user_id: viewer.id.clone(),
                    last_read_message_id: message.id,
                    read_at,
                };
                state.hub.broadcast_to_room(message.room_id, &event);
            }
        }
        return render_new_message(state, message, viewer).await;
    }
    if message.user_id == viewer.id {
        return None;
    }
    // Muted rooms suppress the sidebar unread bump for background recipients.
    // The foreground branch above is intentionally left untouched: viewers
    // with the room open still see new messages and still advance their read
    // watermark.
    let mode = db::notifications::room_mute_mode(&state.chat, &viewer.id, message.room_id)
        .await
        .unwrap_or(db::notifications::MuteMode::None);
    if !mode.allows_unread_bump() {
        return None;
    }
    let room = db::chat::get_room(&state.chat, message.room_id)
        .await
        .ok()??;
    render_unread_badge(state, viewer, &room).await
}

async fn render_reaction_bar(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let counts = db::chat::list_reactions(&state.chat, message_id, user_id)
        .await
        .ok()?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    ReactionUpdateFragment {
        message_id,
        reactions: &reactions,
    }
    .render()
    .ok()
}

/// Build a MessageView for a freshly broadcast message rendered for `viewer`.
/// Reactions are empty (a brand-new message has none); can_edit/can_delete
/// reflect viewer's role and authorship.
async fn render_new_message(
    state: &AppState,
    message: &models::Message,
    viewer: &User,
) -> Option<String> {
    let can_edit = message.user_id == viewer.id;
    let can_delete =
        message.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
    let prior = db::chat::prior_message_in_room(&state.chat, message.room_id, message.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (message.user_id.as_str(), message.created_at.as_str()),
    );
    let meta = super::load_author_meta(state, &message.user_id, &viewer.id)
        .await
        .ok();
    let (display_name, avatar_ext, status, custom_status) = match meta {
        Some(m) => (m.display_name, m.avatar_ext, m.status, m.custom_status),
        None => (None, None, db::auth::STATUS_ACTIVE.to_string(), None),
    };
    let attachments = db::uploads::attachments_for_message(&state.chat, message.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[message.id])
        .await
        .ok()
        .and_then(|mut m| m.remove(&message.id))
        .unwrap_or_default();
    let view = MessageView {
        id: message.id,
        room_id: message.room_id,
        user_id: message.user_id.clone(),
        username: message.author_name.clone(),
        display_name,
        avatar_ext,
        status,
        custom_status,
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
        reply_count: 0,
        parent_id: message.parent_id,
        attachments,
        mentions,
        is_pinned: false,
    };
    NewMessageFragment { message: &view }.render().ok()
}

/// Re-fetch the edited message and render the per-viewer outerHTML OOB swap.
/// Loading from the DB ensures the broadcast picks up the canonical body and
/// edited_at timestamp rather than trusting the event payload.
async fn render_edited_message(state: &AppState, message_id: i64, viewer: &User) -> Option<String> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await
        .ok()??;
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &m.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let meta = super::load_author_meta(state, &m.user_id, &viewer.id)
        .await
        .ok()?;
    let counts = db::chat::list_reactions(&state.chat, m.id, &viewer.id)
        .await
        .ok()?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView {
            emoji: r.emoji,
            count: r.count,
            viewer_reacted: r.reacted_by_me,
        })
        .collect();
    let prior = db::chat::prior_message_in_room(&state.chat, m.room_id, m.id)
        .await
        .ok()
        .flatten();
    let is_follow_up = db::chat::is_follow_up_of(
        prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (m.user_id.as_str(), m.created_at.as_str()),
    );
    let can_edit = m.user_id == viewer.id;
    let can_delete = m.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
    let reply_count = db::chat::count_replies(&state.chat, m.id).await.ok()?;
    let parent_id = m.parent_id;
    let attachments = db::uploads::attachments_for_message(&state.chat, m.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
        .await
        .ok()
        .and_then(|mut x| x.remove(&m.id))
        .unwrap_or_default();
    let view = MessageView {
        id: m.id,
        room_id: m.room_id,
        user_id: m.user_id,
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
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
        show_unread_divider: false,
        reply_count,
        parent_id,
        attachments,
        mentions,
        is_pinned: false,
    };
    EditedMessageFragment { message: &view }.render().ok()
}

/// Render a thread reply for `viewer` along with an OOB update of the
/// parent's reply-count pill in the main feed. The reply fragment targets
/// `#thread-replies-{parent_id}`, which only exists in the DOM when the
/// viewer has the thread panel open for that parent - HTMX silently drops
/// the swap otherwise. The reply-count fragment targets `#reply-count-{parent_id}`,
/// which lives under every top-level message in the main feed.
async fn render_thread_reply(
    state: &AppState,
    parent_id: i64,
    message: &models::Message,
    viewer: &User,
) -> Option<String> {
    if db::auth::is_blocked_either_way(&state.auth, &viewer.id, &message.user_id)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let meta = super::load_author_meta(state, &message.user_id, &viewer.id)
        .await
        .ok()?;
    let attachments = db::uploads::attachments_for_message(&state.chat, message.id)
        .await
        .ok()
        .unwrap_or_default();
    let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[message.id])
        .await
        .ok()
        .and_then(|mut x| x.remove(&message.id))
        .unwrap_or_default();
    let view = MessageView {
        id: message.id,
        room_id: message.room_id,
        user_id: message.user_id.clone(),
        username: meta.username,
        display_name: meta.display_name,
        avatar_ext: meta.avatar_ext,
        status: meta.status,
        custom_status: meta.custom_status,
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit: false,
        can_delete: false,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        reply_count: 0,
        parent_id: Some(parent_id),
        attachments,
        mentions,
        is_pinned: false,
    };
    let mut html = ThreadReplyOobFragment {
        parent_id,
        message: &view,
    }
    .render()
    .ok()?;
    let count = db::chat::count_replies(&state.chat, parent_id).await.ok()?;
    let pill = ReplyCountFragment {
        message_id: parent_id,
        room_id: message.room_id,
        reply_count: count,
        oob: true,
    }
    .render()
    .ok()?;
    html.push_str(&pill);
    Some(html)
}

/// Render a `Mentioned` event for the connected user. The hub's
/// `broadcast_to_user` already filters by id; the additional guard in the
/// caller is a belt-and-braces self-check.
fn render_mentioned(event: &ChatEvent) -> Option<String> {
    let ChatEvent::Mentioned {
        kind,
        room_id,
        room_type,
        room_label,
        message_id,
        author_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return None;
    };
    MentionedFragment {
        kind,
        room_id: *room_id,
        room_type,
        room_label,
        message_id: *message_id,
        author_label,
        snippet,
        target_path,
    }
    .render()
    .ok()
}

fn render_mention_cleared(event: &ChatEvent) -> Option<String> {
    let ChatEvent::MentionCleared {
        room_id,
        message_id,
        ..
    } = event
    else {
        return None;
    };
    MentionClearedFragment {
        room_id: *room_id,
        message_id: *message_id,
    }
    .render()
    .ok()
}

/// Render a full sidebar OOB replacement for `viewer` reflecting current
/// room/DM membership and unread counts. Used to live-update the sidebar
/// when membership changes (new DM, room kick, room invite).
async fn render_sidebar(state: &AppState, viewer: &User) -> Option<String> {
    // Live OOB sidebar refreshes only fire from DM-creation today, so render
    // the Home (DM-only) variant. When per-enclave events ship OOB rendering,
    // they will pass current_enclave themselves.
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(state, viewer, None).await.ok()?;
    SidebarUpdateFragment {
        user: viewer,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    }
    .render()
    .ok()
}

/// Build the OOB-tagged pinned strip for `viewer`'s context in `room_id`.
/// Picks the right URL (room vs DM) based on the room's type and the
/// viewer's perspective so the "See all (N) pinned" link in the
/// broadcast points where the receiving tab expects to navigate.
async fn render_pinned_strip(state: &AppState, viewer: &User, room_id: i64) -> Option<String> {
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;
    let pin_path = if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room_id, &viewer.id)
            .await
            .ok()??;
        format!("/dm/{peer_id}/pins")
    } else {
        format!("/room/{room_id}/pins")
    };
    super::pinned::build_strip_fragment(state, room_id, pin_path, true)
        .await
        .ok()?
        .render()
        .ok()
}
