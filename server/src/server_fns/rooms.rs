use dioxus::prelude::*;

/// Join a private room using an invite code.
/// Returns the room_id on success so the caller can navigate there.
#[server]
pub async fn join_room_by_invite(invite_code: String) -> Result<i64, ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;

    let room = crate::db::chat::get_room_by_invite(pool, &invite_code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Invalid invite code"))?;

    if room.room_type != "private" {
        return Err(ServerFnError::new("Invalid invite code"));
    }

    let already = crate::db::chat::is_room_member(pool, room.id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    if !already {
        crate::db::chat::add_room_member(pool, room.id, &user.id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

        let event = crate::ws::events::ChatEvent::RoomMemberAdded {
            room_id: room.id,
            user_id: user.id.clone(),
        };
        crate::ws::hub::get_hub().broadcast_global(&event);
    }

    Ok(room.id)
}

/// Leave a private room (removes own membership).
#[server]
pub async fn leave_room(room_id: i64) -> Result<(), ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;

    crate::db::chat::remove_room_member(pool, room_id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::RoomMemberRemoved {
        room_id,
        user_id: user.id.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);

    Ok(())
}
