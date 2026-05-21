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
use crate::views::render_template;
use crate::views::room::ReplyCountFragment;
use crate::views::room::{MessageView, ReactionView};
use crate::views::ws_fragments::{
    render_event, CallSignalFragment, EditedMessageFragment, MentionClearedFragment,
    MentionedFragment, NewMessageFragment, ReactionUpdateFragment, ReminderFragment,
    SeenIndicatorFragment, SidebarUpdateFragment, ThreadReplyOobFragment, UnreadBadgeFragment,
    VoiceEventFragment,
};
use crate::ws::events::ChatEvent;
use crate::ws::hub::{ConnId, RingingResult};

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "subscribe")]
    Subscribe { room_id: i64 },
    #[serde(rename = "typing")]
    Typing { room_id: i64 },
    #[serde(rename = "thread_typing")]
    ThreadTyping { room_id: i64, parent_id: i64 },
    /// WebRTC 1:1 call signaling. Relayed verbatim to the other member of
    /// the DM room; the server validates membership and never inspects
    /// `payload`. See [`ChatEvent::CallSignal`].
    #[serde(rename = "call_signal")]
    CallSignal {
        room_id: i64,
        kind: String,
        #[serde(default)]
        payload: Option<String>,
    },
    /// Join an enclave voice channel (`room_type = 'voice'`).
    #[serde(rename = "voice_join")]
    VoiceJoin { room_id: i64 },
    /// Leave the voice channel this connection is in.
    #[serde(rename = "voice_leave")]
    VoiceLeave { room_id: i64 },
    /// Mesh signaling to a specific peer in a voice channel. See
    /// [`ChatEvent::VoiceSignal`].
    #[serde(rename = "voice_signal")]
    VoiceSignal {
        room_id: i64,
        target_user_id: String,
        kind: String,
        #[serde(default)]
        payload: Option<String>,
    },
}

/// Recognized `kind` discriminators for a call signal. Anything else is
/// dropped before relay so a malformed client cannot inject arbitrary
/// values into a peer's call state machine.
const CALL_SIGNAL_KINDS: &[&str] = &[
    "invite", "offer", "answer", "ice", "accept", "reject", "cancel", "hangup",
];

/// Recognized `kind` discriminators for a voice-channel mesh signal.
const VOICE_SIGNAL_KINDS: &[&str] = &["offer", "answer", "ice"];

/// Upper bound on a relayed signaling payload. An SDP blob is a few KiB;
/// 64 KiB leaves generous headroom while bounding what one peer can push
/// through the relay in a single frame.
const MAX_CALL_PAYLOAD: usize = 64 * 1024;

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
    // Record that the in-app notification surface is alive for this user.
    // Bumped again, throttled, on each outbound Mentioned frame in the send
    // task below. Consumed by the email-digest "missed" predicate alongside
    // `last_active_at`; does not influence idle-flip.
    db::auth::bump_last_ws_seen(&state.auth, &user.id).await;
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
        // Per-connection throttle for `last_ws_seen_at` bumps. The
        // connection-open bump just happened in the parent handler, so the
        // next bump is allowed in WS_BUMP_THROTTLE from now.
        const WS_BUMP_THROTTLE: Duration = Duration::from_secs(300);
        let mut last_ws_bump: std::time::Instant = std::time::Instant::now();
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
                                ChatEvent::PollUpdated { message_id, .. } => {
                                    render_poll(&send_state, *message_id, &send_user.id).await
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
                                        // The in-app surface is about to fire
                                        // for this user. Record it for the
                                        // digest, throttled per-connection so
                                        // bursty rooms do not amplify writes.
                                        if last_ws_bump.elapsed() >= WS_BUMP_THROTTLE {
                                            db::auth::bump_last_ws_seen(
                                                &send_state.auth,
                                                &send_user.id,
                                            )
                                            .await;
                                            last_ws_bump = std::time::Instant::now();
                                        }
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
                                // LC-63: reminder toast. No mute check - the
                                // user explicitly asked to be pinged.
                                ChatEvent::Reminder { user_id, .. }
                                    if user_id == &send_user.id =>
                                {
                                    render_reminder(&e)
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
                                ChatEvent::SidebarCategoriesChanged { .. } => {
                                    // Shared category state changed in an
                                    // enclave this user is a member of.
                                    // Re-render their sidebar so live tabs
                                    // pick up the change without a manual
                                    // refresh. The fragment uses None for
                                    // current_enclave (DM-only shape); WS
                                    // tabs on /enclave/N will see the room
                                    // section render via the normal page
                                    // navigation flow when the user moves.
                                    render_sidebar(&send_state, &send_user).await
                                }
                                ChatEvent::MessagePinned {
                                    room_id,
                                    message_id,
                                    ..
                                } => {
                                    // Fan out: re-render this viewer's
                                    // bubble OOB (so its hover menu flips
                                    // Pin -> Unpin) and the pinned-strip
                                    // OOB. Per-viewer because can_edit/
                                    // can_delete and the strip's "See all"
                                    // URL depend on identity.
                                    render_pin_event(
                                        &send_state,
                                        &send_user,
                                        *room_id,
                                        *message_id,
                                        true,
                                    )
                                    .await
                                }
                                ChatEvent::MessageUnpinned {
                                    room_id,
                                    message_id,
                                } => render_pin_event(
                                    &send_state,
                                    &send_user,
                                    *room_id,
                                    *message_id,
                                    false,
                                )
                                .await,
                                ChatEvent::CallSignal { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_call_signal(&e)
                                }
                                ChatEvent::VoiceJoined { .. } | ChatEvent::VoiceLeft { .. } => {
                                    render_voice_event(&e)
                                }
                                ChatEvent::VoiceRoster { to_user_id, .. }
                                | ChatEvent::VoiceSignal { to_user_id, .. }
                                    if to_user_id == &send_user.id =>
                                {
                                    render_voice_event(&e)
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
                        ClientFrame::CallSignal {
                            room_id,
                            kind,
                            payload,
                        } => {
                            relay_call_signal(&state, &user, &username, room_id, &kind, payload)
                                .await;
                        }
                        ClientFrame::VoiceJoin { room_id } => {
                            handle_voice_join(&state, &user, &username, conn_id, room_id).await;
                        }
                        ClientFrame::VoiceLeave { room_id } => {
                            let _ = room_id;
                            handle_voice_leave(&state, conn_id);
                        }
                        ClientFrame::VoiceSignal {
                            room_id,
                            target_user_id,
                            kind,
                            payload,
                        } => {
                            relay_voice_signal(
                                &state,
                                &user,
                                &username,
                                conn_id,
                                room_id,
                                &target_user_id,
                                &kind,
                                payload,
                            );
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    handle_voice_leave(&state, conn_id);
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

/// LC-66: re-render a poll block for `user_id` (keeps per-viewer
/// `voted_by_me` highlighting correct). Returns None if the message is not
/// a poll.
async fn render_poll(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let view = crate::views::room::build_poll_view(&state.chat, &state.auth, message_id, user_id)
        .await
        .ok()??;
    crate::views::room::PollUpdateFragment { poll: &view }
        .render()
        .ok()
}

async fn render_reaction_bar(state: &AppState, message_id: i64, user_id: &str) -> Option<String> {
    let m = db::chat::get_message(&state.chat, message_id)
        .await
        .ok()??;
    let emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id)
        .await
        .ok()
        .unwrap_or_default();
    let counts = db::chat::list_reactions(&state.chat, message_id, user_id)
        .await
        .ok()?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView::new(r.emoji, r.count, r.reacted_by_me, &emojis))
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
    let can_delete = message.user_id == viewer.id
        || db::room_rbac::is_room_moderator(&state.chat, message.room_id, &viewer.id, &viewer.role)
            .await
            .unwrap_or(false);
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
    let meta = super::resolve_msg_author(state, &message.user_id, message.webhook_id, &viewer.id)
        .await
        .ok();
    let (
        display_name,
        avatar_ext,
        status,
        custom_status,
        author_is_bot,
        author_is_webhook,
        webhook_avatar_url,
    ) = match meta {
        Some(m) => (
            m.display_name,
            m.avatar_ext,
            m.status,
            m.custom_status,
            m.is_bot,
            m.is_webhook,
            m.avatar_url,
        ),
        None => (
            None,
            None,
            db::auth::STATUS_ACTIVE.to_string(),
            None,
            false,
            false,
            None,
        ),
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
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, message.room_id)
        .await
        .ok()
        .unwrap_or_default();
    let quote_preview = match message.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid)
            .await
            .ok()
            .flatten(),
        None => None,
    };
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
        is_bookmarked: false,
        custom_emojis,
        quote_preview,
        is_system: message.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, message.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        author_is_bot,
        author_is_webhook,
        webhook_avatar_url,
    };
    render_template(&NewMessageFragment { message: &view }).ok()
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
    let meta = super::resolve_msg_author(state, &m.user_id, m.webhook_id, &viewer.id)
        .await
        .ok()?;
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id)
        .await
        .ok()
        .unwrap_or_default();
    let counts = db::chat::list_reactions(&state.chat, m.id, &viewer.id)
        .await
        .ok()?;
    let reactions: Vec<ReactionView> = counts
        .into_iter()
        .map(|r| ReactionView::new(r.emoji, r.count, r.reacted_by_me, &custom_emojis))
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
    let can_delete = m.user_id == viewer.id
        || db::room_rbac::is_room_moderator(&state.chat, m.room_id, &viewer.id, &viewer.role)
            .await
            .unwrap_or(false);
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
    let pinned_ids = db::pinned::pinned_message_ids_for_room(&state.chat, m.room_id)
        .await
        .ok()
        .unwrap_or_default();
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &viewer.id, m.id)
        .await
        .unwrap_or(false);
    let quote_preview = match m.quote_id {
        Some(qid) => crate::views::room::build_quote_preview(&state.chat, &state.auth, qid)
            .await
            .ok()
            .flatten(),
        None => None,
    };
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
        is_pinned: pinned_ids.contains(&m.id),
        is_bookmarked,
        custom_emojis,
        quote_preview,
        is_system: m.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, m.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        author_is_bot: meta.is_bot,
        author_is_webhook: meta.is_webhook,
        webhook_avatar_url: meta.avatar_url.clone(),
    };
    render_template(&EditedMessageFragment { message: &view }).ok()
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
    let meta = super::resolve_msg_author(state, &message.user_id, message.webhook_id, &viewer.id)
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
    let custom_emojis = db::custom_emojis::refs_for_room(&state.chat, message.room_id)
        .await
        .ok()
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
        is_bookmarked: false,
        custom_emojis,
        quote_preview: None,
        is_system: message.is_system,
        poll: crate::views::room::build_poll_view(&state.chat, &state.auth, message.id, &viewer.id)
            .await
            .ok()
            .flatten(),
        author_is_bot: meta.is_bot,
        author_is_webhook: meta.is_webhook,
        webhook_avatar_url: meta.avatar_url.clone(),
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

/// Render a `Reminder` event for the connected user (LC-63).
fn render_reminder(event: &ChatEvent) -> Option<String> {
    let ChatEvent::Reminder {
        room_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return None;
    };
    ReminderFragment {
        room_label,
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

/// Render an inbound `CallSignal` into the `#lc-call-bus` OOB fragment the
/// browser's call state machine consumes.
fn render_call_signal(event: &ChatEvent) -> Option<String> {
    let ChatEvent::CallSignal {
        room_id,
        from_user_id,
        from_name,
        kind,
        payload,
        ..
    } = event
    else {
        return None;
    };
    CallSignalFragment {
        room_id: *room_id,
        from_user_id,
        from_name,
        kind,
        payload: payload.as_deref(),
    }
    .render()
    .ok()
}

/// Validate and relay one WebRTC call signal to the other member of a DM
/// room. The server is a dumb relay: it confirms the sender belongs to a
/// `dm` room, that the two parties have not blocked each other, and that
/// the `kind`/`payload` are within bounds, then forwards the opaque blob.
///
/// Media kinds (`invite`/`offer`/`answer`/`ice`) go only to the peer.
/// Control kinds (`accept`/`reject`/`cancel`/`hangup`) are additionally
/// echoed to the sender's other tabs so a call answered or ended on one
/// device tears down the ringing UI on the rest.
async fn relay_call_signal(
    state: &AppState,
    user: &User,
    from_name: &str,
    room_id: i64,
    kind: &str,
    payload: Option<String>,
) {
    if !CALL_SIGNAL_KINDS.contains(&kind) {
        return;
    }
    if payload.as_ref().is_some_and(|p| p.len() > MAX_CALL_PAYLOAD) {
        return;
    }
    let room = match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) if r.room_type == "dm" => r,
        _ => return,
    };
    let members = match db::chat::list_room_member_ids(&state.chat, room_id).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if !members.iter().any(|id| id == &user.id) {
        return;
    }
    let Some(peer_id) = members.into_iter().find(|id| id != &user.id) else {
        return;
    };
    // A blocked relationship in either direction kills signaling outright.
    // `unwrap_or(true)` fails closed: a lookup error drops the signal.
    if db::auth::is_blocked_either_way(&state.auth, &user.id, &peer_id)
        .await
        .unwrap_or(true)
    {
        return;
    }
    // An `invite` claims the per-DM ringing slot. On glare (the peer is
    // already ringing us) we replay the winner's invite back to the second
    // caller so their UI flips from outgoing to incoming, and never deliver
    // the second invite to the peer. Terminating signals release the slot.
    if kind == "invite" {
        match state
            .hub
            .try_start_ringing(room_id, &user.id, from_name, payload.clone())
        {
            RingingResult::Started => {
                let event = ChatEvent::CallSignal {
                    room_id,
                    to_user_id: peer_id.clone(),
                    from_user_id: user.id.clone(),
                    from_name: from_name.to_string(),
                    kind: kind.to_string(),
                    payload: payload.clone(),
                };
                state.hub.broadcast_to_user(&peer_id, &event);
                post_call_started_message(state, user, &room).await;
            }
            RingingResult::DuplicateSelf => {
                // Same caller already holds the slot. Drop silently so the
                // peer is not invited a second time and a duplicate "started
                // a call" system message is not posted.
            }
            RingingResult::Glare {
                winner_id,
                from_name: winner_name,
                payload: winner_payload,
            } => {
                let event = ChatEvent::CallSignal {
                    room_id,
                    to_user_id: user.id.clone(),
                    from_user_id: winner_id,
                    from_name: winner_name,
                    kind: "invite".to_string(),
                    payload: winner_payload,
                };
                state.hub.broadcast_to_user(&user.id, &event);
            }
        }
        return;
    }

    if matches!(kind, "accept" | "reject" | "cancel" | "hangup") {
        state.hub.clear_ringing(room_id);
    }

    let mut recipients: Vec<String> = vec![peer_id];
    if matches!(kind, "accept" | "reject" | "cancel" | "hangup") {
        recipients.push(user.id.clone());
    }
    for to in recipients {
        let event = ChatEvent::CallSignal {
            room_id,
            to_user_id: to.clone(),
            from_user_id: user.id.clone(),
            from_name: from_name.to_string(),
            kind: kind.to_string(),
            payload: payload.clone(),
        };
        state.hub.broadcast_to_user(&to, &event);
    }
}

/// Insert a "started a call" message into the DM `room` authored by `user`,
/// then broadcast it like any normal message so it lands in both members'
/// open conversation and bumps the sidebar for anyone not viewing the room.
async fn post_call_started_message(state: &AppState, user: &User, room: &models::Room) {
    let new_id =
        match db::chat::insert_system_message(&state.chat, room.id, &user.id, "started a call.")
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "failed to insert call-started message");
                return;
            }
        };
    let raw = match db::chat::get_message(&state.chat, new_id).await {
        Ok(Some(r)) => r,
        _ => return,
    };
    let message = models::Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name: user.username.clone(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
    };
    let event = ChatEvent::NewMessage {
        message,
        is_dm: true,
    };
    if let Err(e) = super::broadcast_room_message(state, room, &event).await {
        tracing::warn!(error = %e, "failed to broadcast call-started message");
    }
}

/// Render a voice-channel event into the `#lc-voice-bus` OOB fragment the
/// browser's voice mesh consumes.
fn render_voice_event(event: &ChatEvent) -> Option<String> {
    match event {
        ChatEvent::VoiceJoined {
            room_id,
            user_id,
            username,
        } => VoiceEventFragment {
            room_id: *room_id,
            kind: "joined",
            user_id,
            username,
            peers_json: "",
            payload: None,
        }
        .render()
        .ok(),
        ChatEvent::VoiceLeft { room_id, user_id } => VoiceEventFragment {
            room_id: *room_id,
            kind: "left",
            user_id,
            username: "",
            peers_json: "",
            payload: None,
        }
        .render()
        .ok(),
        ChatEvent::VoiceRoster { room_id, peers, .. } => {
            let json = serde_json::to_string(peers).ok()?;
            VoiceEventFragment {
                room_id: *room_id,
                kind: "roster",
                user_id: "",
                username: "",
                peers_json: &json,
                payload: None,
            }
            .render()
            .ok()
        }
        ChatEvent::VoiceSignal {
            room_id,
            from_user_id,
            from_name,
            kind,
            payload,
            ..
        } => VoiceEventFragment {
            room_id: *room_id,
            kind,
            user_id: from_user_id,
            username: from_name,
            peers_json: "",
            payload: payload.as_deref(),
        }
        .render()
        .ok(),
        _ => None,
    }
}

/// Handle a `voice_join` frame: validate the room is an accessible voice
/// channel, register the connection in the hub, hand the joiner the current
/// roster, and announce the joiner to everyone already in the channel.
async fn handle_voice_join(
    state: &AppState,
    user: &User,
    username: &str,
    conn_id: ConnId,
    room_id: i64,
) {
    match db::chat::get_room(&state.chat, room_id).await {
        Ok(Some(r)) if r.is_voice => {}
        _ => return,
    }
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let peers = state.hub.voice_join(conn_id, room_id);
    // The joiner gets the roster so it can open a peer connection to each.
    state.hub.broadcast_to_user(
        &user.id,
        &ChatEvent::VoiceRoster {
            room_id,
            to_user_id: user.id.clone(),
            peers,
        },
    );
    // Existing voice members learn about the joiner and await its offer (the
    // newest joiner is always the mesh offerer). The event also fans out to
    // anyone just *viewing* the voice page so their participants preview
    // stays in sync without waiting for a page reload.
    let joined = ChatEvent::VoiceJoined {
        room_id,
        user_id: user.id.clone(),
        username: username.to_string(),
    };
    state.hub.broadcast_to_room(room_id, &joined);
}

/// Remove `conn_id` from its voice channel (if any) and tell the remaining
/// participants. Idempotent - safe to call on both explicit leave and
/// disconnect.
fn handle_voice_leave(state: &AppState, conn_id: ConnId) {
    let Some((room_id, user_id, _)) = state.hub.voice_leave(conn_id) else {
        return;
    };
    // Tell every subscriber of the room, not just remaining voice members: a
    // viewer who is *not* in the call still needs the update so their
    // participants preview stays accurate.
    let left = ChatEvent::VoiceLeft { room_id, user_id };
    state.hub.broadcast_to_room(room_id, &left);
}

/// Relay one mesh signal (offer/answer/ice) to a specific peer in the same
/// voice channel. The server validates channel co-membership and bounds the
/// payload; it never interprets the SDP/ICE blob.
#[allow(clippy::too_many_arguments)]
fn relay_voice_signal(
    state: &AppState,
    user: &User,
    from_name: &str,
    conn_id: ConnId,
    room_id: i64,
    target_user_id: &str,
    kind: &str,
    payload: Option<String>,
) {
    if !VOICE_SIGNAL_KINDS.contains(&kind) {
        return;
    }
    if payload.as_ref().is_some_and(|p| p.len() > MAX_CALL_PAYLOAD) {
        return;
    }
    if !state.hub.is_in_voice_room(conn_id, room_id) {
        return;
    }
    if !state
        .hub
        .voice_room_users(room_id)
        .iter()
        .any(|u| u == target_user_id)
    {
        return;
    }
    state.hub.broadcast_to_user(
        target_user_id,
        &ChatEvent::VoiceSignal {
            room_id,
            to_user_id: target_user_id.to_string(),
            from_user_id: user.id.clone(),
            from_name: from_name.to_string(),
            kind: kind.to_string(),
            payload,
        },
    );
}

/// Render a full sidebar OOB replacement for `viewer` reflecting current
/// room/DM membership and unread counts. Used to live-update the sidebar
/// when membership changes (new DM, room kick, room invite).
async fn render_sidebar(state: &AppState, viewer: &User) -> Option<String> {
    // Live OOB sidebar refreshes only fire from DM-creation today, so render
    // the Home (DM-only) variant. When per-enclave events ship OOB rendering,
    // they will pass current_enclave themselves.
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_sidebar(state, viewer, None).await.ok()?;
    SidebarUpdateFragment {
        user: viewer,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    }
    .render()
    .ok()
}

/// Build the OOB fragments for a MessagePinned / MessageUnpinned event:
/// the re-rendered message bubble (OOB so its hover menu flips for this
/// viewer) followed by the pinned strip (also OOB). Returns the two
/// fragments concatenated in one Text frame. The bubble OOB swap is a
/// no-op for subscribers who don't currently have `#msg-{id}` in their
/// DOM (e.g. the message scrolled off, or this viewer is on a different
/// page) - htmx silently drops unmatched OOB targets. If the message has
/// been deleted between the mutation and the broadcast, the bubble part
/// is skipped and only the strip is emitted.
async fn render_pin_event(
    state: &AppState,
    viewer: &User,
    room_id: i64,
    message_id: i64,
    is_pinned: bool,
) -> Option<String> {
    use askama::Template;
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;
    let pin_path = if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room_id, &viewer.id)
            .await
            .ok()??;
        format!("/dm/{peer_id}/pins")
    } else {
        format!("/room/{room_id}/pins")
    };
    let strip_html = super::pinned::build_strip_fragment(state, room_id, pin_path, true)
        .await
        .ok()?
        .render()
        .ok()?;
    let is_bookmarked = db::bookmarks::is_bookmarked(&state.chat, &viewer.id, message_id)
        .await
        .unwrap_or(false);
    let bubble_html = match super::load_message_view_for_viewer(
        state,
        viewer,
        message_id,
        is_pinned,
        is_bookmarked,
    )
    .await
    {
        Ok(view) => crate::views::room::SingleMessageFragment {
            message: &view,
            oob: true,
        }
        .render()
        .ok()
        .unwrap_or_default(),
        Err(_) => String::new(),
    };
    Some(format!("{bubble_html}{strip_html}"))
}
