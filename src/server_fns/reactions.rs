use dioxus::prelude::*;

use crate::models::Reaction;

fn validate_emoji(emoji: &str) -> Result<(), ServerFnError> {
    let count = emoji.chars().count();
    if count == 0 || count > 8 {
        return Err(ServerFnError::new("Invalid emoji"));
    }
    if emoji.chars().any(|c| c.is_control()) {
        return Err(ServerFnError::new("Invalid emoji"));
    }
    Ok(())
}

/// Toggle a reaction on a message (add if not present, remove if present).
/// Returns the updated reaction list for that message.
#[server]
pub async fn toggle_reaction(
    message_id: i64,
    emoji: String,
) -> Result<Vec<Reaction>, ServerFnError> {
    validate_emoji(&emoji)?;

    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;

    // Verify the message exists and get its room_id for the WS broadcast.
    let msg = crate::db::chat::get_message(pool, message_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;

    let added = crate::db::chat::toggle_reaction(pool, message_id, &user.id, &emoji)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = if added {
        crate::ws::events::ChatEvent::ReactionAdded {
            message_id,
            room_id: msg.room_id,
            emoji: emoji.clone(),
            user_id: user.id.clone(),
        }
    } else {
        crate::ws::events::ChatEvent::ReactionRemoved {
            message_id,
            room_id: msg.room_id,
            emoji: emoji.clone(),
            user_id: user.id.clone(),
        }
    };
    crate::ws::hub::get_hub().broadcast_to_room(msg.room_id, &event);

    crate::db::chat::list_reactions(pool, message_id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Fetch reactions for a message.
#[server]
pub async fn list_reactions(message_id: i64) -> Result<Vec<Reaction>, ServerFnError> {
    let user = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::list_reactions(pool, message_id, &user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
