use dioxus::prelude::*;

use crate::models::{Message, Room};

#[cfg(not(target_arch = "wasm32"))]
async fn require_room_access(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    is_admin: bool,
) -> Result<(), ServerFnError> {
    if is_admin {
        return Ok(());
    }
    let room = crate::db::chat::get_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Room not found"))?;
    if room.room_type == "public" {
        return Ok(());
    }
    let member = crate::db::chat::is_room_member(pool, room_id, user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if member {
        Ok(())
    } else {
        Err(ServerFnError::new("Access denied"))
    }
}

#[server]
pub async fn list_rooms() -> Result<Vec<Room>, ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_rooms(pool, &user.id, is_admin)
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

    let user = crate::server_fns::helpers::require_auth().await?;
    let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
    require_room_access(chat_pool, room_id, &user.id, is_admin).await?;

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
            edited_at: rm.edited_at,
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

    let user = crate::server_fns::helpers::require_auth().await?;

    // Check mute status
    if user.is_muted {
        if let Some(ref until) = user.muted_until {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            if until.as_str() > now.as_str() {
                return Err(ServerFnError::new(format!("You are muted until {}", until)));
            }
        } else {
            return Err(ServerFnError::new("You are muted"));
        }
    }

    let chat_pool = crate::db::get_chat_pool().await;
    let is_admin = crate::server_fns::helpers::role_level(&user.role) >= 3;
    require_room_access(chat_pool, room_id, &user.id, is_admin).await?;

    let msg_id = crate::db::chat::insert_message(chat_pool, room_id, &user.id, &body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Broadcast via WebSocket
    let author_name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let event = crate::ws::events::ChatEvent::NewMessage {
        message: crate::models::Message {
            id: msg_id,
            room_id,
            user_id: user.id.clone(),
            author_name,
            body,
            created_at: now,
            edited_at: None,
        },
        is_dm: false,
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);

    Ok(msg_id)
}

#[server]
pub async fn edit_message(message_id: i64, new_body: String) -> Result<(), ServerFnError> {
    let new_body = new_body.trim().to_string();
    if new_body.is_empty() {
        return Err(ServerFnError::new("Message body cannot be empty"));
    }

    let user = crate::server_fns::helpers::require_auth().await?;

    // Enforce max_message_length from settings
    let settings_pool = crate::db::get_settings_pool().await;
    if let Ok(Some(max_str)) =
        crate::db::settings::get_setting(settings_pool, "max_message_length").await
    {
        if let Ok(max_len) = max_str.parse::<usize>() {
            if new_body.len() > max_len {
                return Err(ServerFnError::new(format!(
                    "Message exceeds maximum length of {} characters",
                    max_len
                )));
            }
        }
    }

    let chat_pool = crate::db::get_chat_pool().await;

    // Fetch the message — returns None if soft-deleted
    let msg = crate::db::chat::get_message(chat_pool, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;

    // Ownership check: must be author OR admin/moderator
    let is_owner = msg.user_id == user.id;
    let is_privileged = crate::server_fns::helpers::role_level(&user.role) >= 2;
    if !is_owner && !is_privileged {
        return Err(ServerFnError::new("Cannot edit another user's message"));
    }

    let edited_at = crate::db::chat::update_message_body(chat_pool, message_id, &new_body)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::MessageEdited {
        message_id,
        room_id: msg.room_id,
        new_body,
        edited_at,
    };
    crate::ws::hub::get_hub().broadcast_to_room(msg.room_id, &event);

    Ok(())
}
