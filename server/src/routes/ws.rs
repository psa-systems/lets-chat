use std::collections::HashSet;
use std::sync::Mutex;
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
    ReadReceiptFragment, SidebarUpdateFragment,
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
    let subscribed: Mutex<HashSet<i64>> = Mutex::new(HashSet::new());
    let (mut tx, mut rx_ws) = socket.split();

    let send_state = state.clone();
    let send_user = user.clone();
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
                                    render_new_message(&send_state, message, &send_user).await
                                }
                                ChatEvent::MessageEdited { message_id, .. } => {
                                    render_edited_message(&send_state, *message_id, &send_user).await
                                }
                                ChatEvent::ReactionAdded { message_id, .. }
                                | ChatEvent::ReactionRemoved { message_id, .. } => {
                                    render_reaction_bar(&send_state, *message_id, &send_user.id).await
                                }
                                ChatEvent::DmRead { user_id, room_id, .. } => {
                                    if user_id == &send_user.id {
                                        render_read_receipt(&send_state, *room_id, &send_user.id).await
                                    } else {
                                        None
                                    }
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

async fn render_read_receipt(state: &AppState, room_id: i64, user_id: &str) -> Option<String> {
    let room = db::chat::get_room(&state.chat, room_id).await.ok()??;
    if room.room_type == "dm" {
        let peer_id = db::chat::get_dm_peer(&state.chat, room_id, user_id)
            .await
            .ok()??;
        ReadReceiptFragment {
            kind: "dm",
            id: &peer_id,
        }
        .render()
        .ok()
    } else {
        let id_str = room_id.to_string();
        ReadReceiptFragment {
            kind: "room",
            id: &id_str,
        }
        .render()
        .ok()
    }
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
    _state: &AppState,
    message: &models::Message,
    viewer: &User,
) -> Option<String> {
    let can_edit = message.user_id == viewer.id;
    let can_delete =
        message.user_id == viewer.id || viewer.role == "admin" || viewer.role == "moderator";
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
