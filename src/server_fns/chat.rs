use dioxus::prelude::*;

use crate::models::{Message, Room};

#[server]
pub async fn list_rooms() -> Result<Vec<Room>, ServerFnError> {
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_rooms(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn get_room(room_id: i64) -> Result<Option<Room>, ServerFnError> {
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::get_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn list_messages(room_id: i64) -> Result<Vec<Message>, ServerFnError> {
    let chat_pool = crate::db::get_chat_pool().await;
    let auth_pool = crate::db::get_auth_pool().await;

    let raw = crate::db::chat::list_messages(chat_pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut messages = Vec::with_capacity(raw.len());
    for rm in raw {
        let author_name = match crate::db::auth::find_user_by_id(auth_pool, &rm.user_id).await {
            Ok(Some(record)) => record
                .display_name
                .unwrap_or_else(|| record.username.clone()),
            _ => rm.user_id.clone(),
        };
        messages.push(Message {
            id: rm.id,
            room_id: rm.room_id,
            user_id: rm.user_id,
            author_name,
            body: rm.body,
            created_at: rm.created_at,
        });
    }

    Ok(messages)
}

#[server]
pub async fn send_message(room_id: i64, body: String) -> Result<i64, ServerFnError> {
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err(ServerFnError::new("message body cannot be empty"));
    }

    // Extract session token from cookie
    let headers: http::HeaderMap = extract().await?;
    let cookie_header = headers
        .get(http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let session_id = cookie_header
        .split(';')
        .find_map(|part| {
            let part = part.trim();
            part.strip_prefix("session=").and_then(|v| {
                let v = v.trim();
                if v.is_empty() { None } else { Some(v.to_string()) }
            })
        })
        .ok_or_else(|| ServerFnError::new("Not authenticated"))?;

    let auth_pool = crate::db::get_auth_pool().await;
    let record = crate::db::auth::get_user_by_session(auth_pool, &session_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Session expired or invalid"))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::chat::insert_message(chat_pool, room_id, &record.id, &body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
