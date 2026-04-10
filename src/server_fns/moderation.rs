use dioxus::prelude::*;
use crate::models::ModAction;

// ─── Ban ──────────────────────────────────────────────────────────────────────

#[server]
pub async fn ban_user(user_id: String, reason: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    let reason_opt: Option<&str> = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim())
    };

    crate::db::auth::ban_user(auth_pool, &user_id, reason_opt)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    crate::db::auth::delete_user_sessions(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "ban",
        &user_id,
        &actor.id,
        reason_opt,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::UserBanned {
        user_id: user_id.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);

    Ok(())
}

// ─── Unban ────────────────────────────────────────────────────────────────────

#[server]
pub async fn unban_user(user_id: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    crate::db::auth::unban_user(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "unban",
        &user_id,
        &actor.id,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─── Suspend ──────────────────────────────────────────────────────────────────

#[server]
pub async fn suspend_user(
    user_id: String,
    until: String,
    reason: String,
) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    let until_dt = if let Ok(hours) = until.trim().parse::<i64>() {
        (chrono::Utc::now() + chrono::Duration::hours(hours))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    } else {
        until.trim().to_string()
    };

    let reason_opt: Option<&str> = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim())
    };

    crate::db::auth::suspend_user(auth_pool, &user_id, &until_dt, reason_opt)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    crate::db::auth::delete_user_sessions(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let metadata = serde_json::json!({ "until": until_dt }).to_string();

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "suspend",
        &user_id,
        &actor.id,
        reason_opt,
        None,
        Some(&metadata),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─── Mute ─────────────────────────────────────────────────────────────────────

#[server]
pub async fn mute_user(
    user_id: String,
    until: String,
    reason: String,
) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    let until_opt: Option<String> = if until.trim().is_empty() {
        None
    } else if let Ok(hours) = until.trim().parse::<i64>() {
        Some(
            (chrono::Utc::now() + chrono::Duration::hours(hours))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        )
    } else {
        Some(until.trim().to_string())
    };

    let reason_opt: Option<&str> = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim())
    };

    crate::db::auth::mute_user(auth_pool, &user_id, until_opt.as_deref(), reason_opt)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "mute",
        &user_id,
        &actor.id,
        reason_opt,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::UserMuted {
        user_id: user_id.clone(),
        muted_until: until_opt.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);

    Ok(())
}

// ─── Unmute ───────────────────────────────────────────────────────────────────

#[server]
pub async fn unmute_user(user_id: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    crate::db::auth::unmute_user(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "unmute",
        &user_id,
        &actor.id,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─── Delete Message ───────────────────────────────────────────────────────────

#[server]
pub async fn delete_message(message_id: i64, reason: String) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let chat_pool = crate::db::get_chat_pool().await;

    let row = sqlx::query("SELECT user_id, room_id FROM messages WHERE id = ? AND deleted_at IS NULL")
        .bind(message_id)
        .fetch_optional(chat_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Message not found"))?;
    let target_user: String = sqlx::Row::get(&row, "user_id");
    let room_id: i64 = sqlx::Row::get(&row, "room_id");

    crate::db::moderation::soft_delete_message(chat_pool, message_id, &actor.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let reason_opt: Option<&str> = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim())
    };

    let metadata = serde_json::json!({ "message_id": message_id }).to_string();

    crate::db::moderation::log_mod_action(
        chat_pool,
        "delete_message",
        &target_user,
        &actor.id,
        reason_opt,
        Some(room_id),
        Some(&metadata),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::MessageDeleted {
        message_id,
        room_id,
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);

    Ok(())
}

// ─── Kick ─────────────────────────────────────────────────────────────────────

#[server]
pub async fn kick_user(
    user_id: String,
    room_id: i64,
    reason: String,
) -> Result<(), ServerFnError> {
    let actor = crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    let target = crate::db::auth::find_user_by_id(auth_pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let actor_level = crate::server_fns::helpers::role_level(&actor.role);
    let target_level = crate::server_fns::helpers::role_level(&target.role);
    if target_level >= actor_level {
        return Err(ServerFnError::new("Cannot moderate a user with equal or higher role"));
    }

    let reason_opt: Option<&str> = if reason.trim().is_empty() {
        None
    } else {
        Some(reason.trim())
    };

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::moderation::log_mod_action(
        chat_pool,
        "kick",
        &user_id,
        &actor.id,
        reason_opt,
        Some(room_id),
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::UserKicked {
        user_id: user_id.clone(),
        room_id,
    };
    crate::ws::hub::get_hub().broadcast_to_room(room_id, &event);

    Ok(())
}

// ─── List Mod Actions ─────────────────────────────────────────────────────────

#[server]
pub async fn list_mod_actions() -> Result<Vec<ModAction>, ServerFnError> {
    crate::server_fns::helpers::require_role("moderator").await?;

    let pool = crate::db::get_chat_pool().await;
    crate::db::moderation::list_mod_actions(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}
