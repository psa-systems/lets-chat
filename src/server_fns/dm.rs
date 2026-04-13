use dioxus::prelude::*;

use crate::models::{Room, User};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerReadState {
    pub last_read_message_id: i64,
    pub read_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DmUnread {
    pub room_id: i64,
    pub count: i64,
}

/// DM room info with the other user's display name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DmConversation {
    pub room_id: i64,
    pub other_user_id: String,
    pub other_user_name: String,
}

/// Get or create a DM room with another user. Returns the room.
#[server]
pub async fn get_or_create_dm(other_user_id: String) -> Result<Room, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;

    if me.id == other_user_id {
        return Err(ServerFnError::new("Cannot DM yourself"));
    }

    let auth_pool = crate::db::get_auth_pool().await;
    let other = crate::db::auth::find_user_by_id(auth_pool, &other_user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let chat_pool = crate::db::get_chat_pool().await;

    // Check for existing DM room
    if let Some(room) = crate::db::chat::find_dm_room(chat_pool, &me.id, &other_user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    {
        return Ok(room);
    }

    // Create new DM room
    let room_name = format!("dm-{}-{}", me.username, other.username);
    crate::db::chat::create_dm_room(chat_pool, &room_name, &me.id, &other_user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// List the current user's DM conversations.
#[server]
pub async fn list_my_dms() -> Result<Vec<DmConversation>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;

    let chat_pool = crate::db::get_chat_pool().await;
    let auth_pool = crate::db::get_auth_pool().await;

    let dm_rooms = crate::db::chat::list_user_dm_rooms(chat_pool, &me.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut conversations = Vec::with_capacity(dm_rooms.len());
    for (room, other_id) in dm_rooms {
        let other_name = match crate::db::auth::find_user_by_id(auth_pool, &other_id).await {
            Ok(Some(record)) => record
                .display_name
                .unwrap_or_else(|| record.username.clone()),
            _ => other_id.clone(),
        };
        conversations.push(DmConversation {
            room_id: room.id,
            other_user_id: other_id,
            other_user_name: other_name,
        });
    }

    Ok(conversations)
}

/// List all users (for DM user picker). Excludes the current user and banned users.
#[server]
pub async fn list_users_for_dm() -> Result<Vec<User>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;

    let pool = crate::db::get_auth_pool().await;
    let records = crate::db::auth::list_users(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(records
        .into_iter()
        .filter(|r| r.id != me.id && !r.is_banned)
        .map(|r| User {
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            role: r.role,
            is_muted: r.is_muted,
            muted_until: r.muted_until,
            is_banned: r.is_banned,
            ban_reason: r.ban_reason,
            banned_until: r.banned_until,
            created_at: r.created_at,
            read_receipts_enabled: r.read_receipts_enabled,
        })
        .collect())
}

/// Send a message in a DM room. Verifies the sender is a member.
#[server]
pub async fn send_dm_message(room_id: i64, body: String) -> Result<i64, ServerFnError> {
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
                return Err(ServerFnError::new(format!(
                    "You are muted until {}",
                    until
                )));
            }
        } else {
            return Err(ServerFnError::new("You are muted"));
        }
    }

    // Verify room is a DM and user is a member
    let chat_pool = crate::db::get_chat_pool().await;
    let room = crate::db::chat::get_room(chat_pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Room not found"))?;

    if room.room_type != "dm" {
        return Err(ServerFnError::new("Not a DM room"));
    }

    // Check membership
    let is_member = sqlx::query("SELECT 1 FROM room_members WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(&user.id)
        .fetch_optional(chat_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some();

    if !is_member {
        return Err(ServerFnError::new("You are not a member of this DM"));
    }

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
        is_dm: true,
    };
    let hub = crate::ws::hub::get_hub();
    hub.broadcast_to_room(room_id, &event);

    // DM members may not be subscribed to the room yet (e.g. recipient hasn't
    // opened the thread). Notify each non-sender member directly so their
    // sidebar gets the new-DM listing and unread badge.
    let members: Vec<String> = sqlx::query_scalar("SELECT user_id FROM room_members WHERE room_id = ?")
        .bind(room_id)
        .fetch_all(chat_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for member_id in &members {
        if member_id != &user.id {
            hub.broadcast_to_user(member_id, &event);
        }
    }

    Ok(msg_id)
}

#[server]
pub async fn mark_dm_read(room_id: i64, message_id: i64) -> Result<(), ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;

    let chat_pool = crate::db::get_chat_pool().await;
    let room = crate::db::chat::get_room(chat_pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Room not found"))?;
    if room.room_type != "dm" {
        return Err(ServerFnError::new("Not a DM"));
    }

    // Verify the message belongs to this room (prevents smuggling a stranger's id)
    let msg = crate::db::chat::get_message(chat_pool, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;
    if msg.room_id != room_id {
        return Err(ServerFnError::new("Message not in this room"));
    }

    let is_member = crate::db::chat::is_room_member(chat_pool, room_id, &me.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !is_member {
        return Err(ServerFnError::new("Not a member of this DM"));
    }

    let read_at = crate::db::chat::upsert_dm_read(chat_pool, &me.id, room_id, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Symmetric-consent broadcast: only if both users have receipts enabled.
    let peer_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM room_members WHERE room_id = ? AND user_id != ? LIMIT 1",
    )
    .bind(room_id)
    .bind(&me.id)
    .fetch_optional(chat_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let auth_pool = crate::db::get_auth_pool().await;
    if me.read_receipts_enabled {
        if let Some(peer_id) = peer_id {
            if let Ok(Some(peer)) = crate::db::auth::find_user_by_id(auth_pool, &peer_id).await {
                if peer.read_receipts_enabled {
                    let event = crate::ws::events::ChatEvent::DmRead {
                        room_id,
                        user_id: me.id.clone(),
                        last_read_message_id: message_id,
                        read_at,
                    };
                    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);
                }
            }
        }
    }

    Ok(())
}

#[server]
pub async fn get_dm_peer_read_state(room_id: i64) -> Result<Option<PeerReadState>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    if !me.read_receipts_enabled {
        return Ok(None);
    }

    let chat_pool = crate::db::get_chat_pool().await;
    let auth_pool = crate::db::get_auth_pool().await;

    let peer_id: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM room_members WHERE room_id = ? AND user_id != ? LIMIT 1",
    )
    .bind(room_id)
    .bind(&me.id)
    .fetch_optional(chat_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(peer_id) = peer_id else { return Ok(None) };

    let peer = crate::db::auth::find_user_by_id(auth_pool, &peer_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if peer.map(|p| !p.read_receipts_enabled).unwrap_or(true) {
        return Ok(None);
    }

    let state = crate::db::chat::get_dm_read_state(chat_pool, &peer_id, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(state.map(|s| PeerReadState {
        last_read_message_id: s.last_read_message_id,
        read_at: s.updated_at,
    }))
}

#[server]
pub async fn list_dm_unread_counts_fn() -> Result<Vec<DmUnread>, ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    let chat_pool = crate::db::get_chat_pool().await;
    let rows = crate::db::chat::list_dm_unread_counts(chat_pool, &me.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(room_id, count)| DmUnread { room_id, count })
        .collect())
}
