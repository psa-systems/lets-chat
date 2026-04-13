use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use http::HeaderMap;

use crate::ws::events::ClientControl;
use crate::ws::hub::get_hub;

/// Extract session ID from request headers (same logic as helpers::get_session_id).
fn session_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(http::header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part: &str = part.trim();
        if let Some(value) = part.strip_prefix("session=") {
            let value: &str = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Axum handler for `/ws` — upgrades to WebSocket after auth check.
pub async fn ws_handler(ws: WebSocketUpgrade, headers: HeaderMap) -> impl IntoResponse {
    let session_id = match session_from_headers(&headers) {
        Some(id) => id,
        None => {
            return (http::StatusCode::UNAUTHORIZED, "Missing session cookie").into_response();
        }
    };

    let pool = crate::db::get_auth_pool().await;
    let user = match crate::db::auth::get_user_by_session(pool, &session_id).await {
        Ok(Some(u)) => u,
        _ => {
            return (http::StatusCode::UNAUTHORIZED, "Invalid session").into_response();
        }
    };

    if user.is_banned {
        return (http::StatusCode::FORBIDDEN, "Account banned").into_response();
    }

    let user_id = user.id.clone();
    let username = user.display_name.clone().unwrap_or_else(|| user.username.clone());

    ws.on_upgrade(move |socket| handle_socket(socket, user_id, username))
}

async fn handle_socket(socket: WebSocket, user_id: String, username: String) {
    let hub = get_hub();
    let chat_pool = crate::db::get_chat_pool().await;
    let (conn_id, mut rx) = hub.connect(&user_id, &username);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Spawn task: forward hub events to the WebSocket + ping every 30s
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
        ping_interval.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            let json = match serde_json::to_string(&event) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Read loop: handle client control frames
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(ctrl) = serde_json::from_str::<ClientControl>(text.as_str()) {
                    match ctrl {
                        ClientControl::Subscribe { room_id } => {
                            // For DM rooms, verify user is a member
                            let allowed = match crate::db::chat::get_room(chat_pool, room_id).await {
                                Ok(Some(room)) if room.room_type == "dm" => {
                                    sqlx::query("SELECT 1 FROM room_members WHERE room_id = ? AND user_id = ?")
                                        .bind(room_id)
                                        .bind(&user_id)
                                        .fetch_optional(chat_pool)
                                        .await
                                        .unwrap_or(None)
                                        .is_some()
                                }
                                _ => true, // Public rooms: allow anyone
                            };
                            if allowed {
                                hub.subscribe(conn_id, room_id);
                            }
                        }
                        ClientControl::Unsubscribe { room_id } => {
                            hub.unsubscribe(conn_id, room_id);
                        }
                        ClientControl::Typing { room_id } => {
                            hub.notify_typing(conn_id, room_id);
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup
    hub.disconnect(conn_id);
    send_task.abort();
}
