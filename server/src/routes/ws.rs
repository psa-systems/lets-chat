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
use crate::models::User;
use crate::state::AppState;
use crate::views::room::ReactionView;
use crate::views::ws_fragments::{render_event, ReactionUpdateFragment, ReadReceiptFragment};
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
    // The inject_user middleware has already validated the session cookie and
    // filtered banned accounts; just project to the connection-local fields.
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
    // Track which rooms this connection has been authorized to subscribe to so
    // typing pings can be gated to authorized rooms only.
    let subscribed: Mutex<HashSet<i64>> = Mutex::new(HashSet::new());
    let (mut tx, mut rx_ws) = socket.split();

    let send_state = state.clone();
    let send_user_id = user.id.clone();
    let send = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.tick().await;
        loop {
            tokio::select! {
                evt = rx.recv() => {
                    match evt {
                        Ok(e) => {
                            let rendered = match &e {
                                ChatEvent::ReactionAdded { message_id, .. }
                                | ChatEvent::ReactionRemoved { message_id, .. } => {
                                    render_reaction_bar(&send_state, *message_id, &send_user_id).await
                                }
                                ChatEvent::DmRead { user_id, room_id, .. } => {
                                    // Per-user filter: only the badge owner's
                                    // tabs receive the clear fragment.
                                    if user_id == &send_user_id {
                                        render_read_receipt(&send_state, *room_id, &send_user_id).await
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
                            // Public rooms: allow. DM/private: verify membership.
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
                            // Only forward typing pings for rooms the connection
                            // is already authorized to subscribe to. This prevents
                            // a client from leaking typing presence into rooms
                            // they cannot view.
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

/// Render the badge-clearing fragment for the badge owner. For non-DM rooms
/// the badge id is `unread-room-{room_id}`; for DM rooms it is
/// `unread-dm-{peer_user_id}` where peer is the OTHER member from the badge
/// owner's perspective. Returns None if the room cannot be resolved or the
/// caller is not a member of a DM.
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

/// Render the per-user reaction-bar fragment for a WS push. Returns None if
/// the message has been deleted or the DB query fails. The result is an
/// out-of-band swap that updates the corresponding `#reactions-{id}` div in
/// every viewer's tab with their own `viewer_reacted` state.
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
