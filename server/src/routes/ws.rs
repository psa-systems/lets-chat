use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum_extra::extract::cookie::CookieJar;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::auth::SESSION_COOKIE;
use crate::db;
use crate::state::AppState;
use crate::views::ws_fragments::render_event;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientFrame {
    #[serde(rename = "subscribe")]
    Subscribe { room_id: i64 },
    #[serde(rename = "typing")]
    Typing { room_id: i64 },
}

/// Per-connection auth context derived from the session cookie.
struct WsUser {
    id: String,
    username: String,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    jar: CookieJar,
) -> impl IntoResponse {
    let token = match jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) {
        Some(t) => t,
        None => return (http::StatusCode::UNAUTHORIZED, "no session").into_response(),
    };
    let record = match db::auth::get_user_by_session(&state.auth, &token).await {
        Ok(Some(u)) if !u.is_banned => u,
        _ => return (http::StatusCode::UNAUTHORIZED, "invalid").into_response(),
    };

    let username = record
        .display_name
        .clone()
        .unwrap_or_else(|| record.username.clone());
    let user = WsUser {
        id: record.id,
        username,
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: WsUser) {
    let (conn_id, mut rx) = state.hub.connect(&user.id, &user.username);
    let (mut tx, mut rx_ws) = socket.split();

    let send = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.tick().await;
        loop {
            tokio::select! {
                evt = rx.recv() => {
                    match evt {
                        Ok(e) => if let Some(html) = render_event(&e) {
                            if tx.send(Message::Text(html.into())).await.is_err() {
                                break;
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
                            }
                        }
                        ClientFrame::Typing { room_id } => {
                            state.hub.notify_typing(conn_id, room_id);
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
