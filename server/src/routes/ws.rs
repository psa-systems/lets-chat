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
use crate::views::room::{MessageView, ReactionView};
use crate::views::ws_fragments::{
    render_event, EditedMessageFragment, NewMessageFragment, ReactionUpdateFragment,
    SeenIndicatorFragment, SidebarUpdateFragment, UnreadBadgeFragment,
};
use crate::ws::events::ChatEvent;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "subscribe")]
    Subscribe { room_id: i64 },
    #[serde(rename = "typing")]
    Typing { room_id: i64 },
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
    let (conn_id, mut rx) = state.hub.connect(&user.id, &username);
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
                    if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
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
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    state.hub.disconnect(conn_id);
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
        }
        .render()
        .ok()
    } else {
        let id_str = room.id.to_string();
        UnreadBadgeFragment {
            kind: "room",
            id: &id_str,
            unread,
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
    let view = MessageView {
        id: message.id,
        user_id: message.user_id.clone(),
        username: message.author_name.clone(),
        created_at: message.created_at.clone(),
        edited_at: message.edited_at.clone(),
        body: message.body.clone(),
        reactions: Vec::new(),
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
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
    let username = db::auth::find_user_by_id(&state.auth, &m.user_id)
        .await
        .ok()?
        .map(|u| u.username)
        .unwrap_or_else(|| "(unknown)".to_string());
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
    let view = MessageView {
        id: m.id,
        user_id: m.user_id,
        username,
        created_at: m.created_at,
        edited_at: m.edited_at,
        body: m.body,
        reactions,
        can_edit,
        can_delete,
        viewer_id: viewer.id.clone(),
        seen_caption: None,
        is_follow_up,
    };
    EditedMessageFragment { message: &view }.render().ok()
}

/// Render a full sidebar OOB replacement for `viewer` reflecting current
/// room/DM membership and unread counts. Used to live-update the sidebar
/// when membership changes (new DM, room kick, room invite).
async fn render_sidebar(state: &AppState, viewer: &User) -> Option<String> {
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(state, viewer).await.ok()?;
    SidebarUpdateFragment {
        user: viewer,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    }
    .render()
    .ok()
}
