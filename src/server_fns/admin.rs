use dioxus::prelude::*;

use crate::models::{InviteCode, Room, SiteSettings, User};

// ─── Settings ────────────────────────────────────────────────────────────────

#[server]
pub async fn get_settings() -> Result<SiteSettings, ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_settings_pool().await;
    let pairs = crate::db::settings::get_all_settings(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let get = |key: &str, default: &str| -> String {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| default.to_string())
    };

    let settings = SiteSettings {
        site_name: get("site_name", "Let's Chat"),
        registration_open: get("registration_open", "true") == "true",
        require_invite_code: get("require_invite_code", "false") == "true",
        default_role: get("default_role", "user"),
        welcome_message: get("welcome_message", ""),
        max_message_length: get("max_message_length", "4000")
            .parse()
            .unwrap_or(4000),
        motd: get("motd", ""),
        maintenance_mode: get("maintenance_mode", "false") == "true",
        rate_limit_messages: get("rate_limit_messages", "30")
            .parse()
            .unwrap_or(30),
        smtp_host: get("smtp_host", ""),
        smtp_port: get("smtp_port", "587"),
        smtp_user: get("smtp_user", ""),
        smtp_pass: get("smtp_pass", ""),
    };

    Ok(settings)
}

#[server]
pub async fn update_settings(settings: SiteSettings) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_settings_pool().await;

    let pairs: &[(&str, String)] = &[
        ("site_name", settings.site_name),
        ("registration_open", settings.registration_open.to_string()),
        ("require_invite_code", settings.require_invite_code.to_string()),
        ("default_role", settings.default_role),
        ("welcome_message", settings.welcome_message),
        ("max_message_length", settings.max_message_length.to_string()),
        ("motd", settings.motd),
        ("maintenance_mode", settings.maintenance_mode.to_string()),
        ("rate_limit_messages", settings.rate_limit_messages.to_string()),
        ("smtp_host", settings.smtp_host),
        ("smtp_port", settings.smtp_port),
        ("smtp_user", settings.smtp_user),
        ("smtp_pass", settings.smtp_pass),
    ];

    for (key, value) in pairs {
        crate::db::settings::set_setting(pool, key, value)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }

    Ok(())
}

// ─── Users ────────────────────────────────────────────────────────────────────

#[server]
pub async fn list_users() -> Result<Vec<User>, ServerFnError> {
    crate::server_fns::helpers::require_role("moderator").await?;

    let pool = crate::db::get_auth_pool().await;
    let records = crate::db::auth::list_users(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(records
        .into_iter()
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

#[server]
pub async fn change_user_role(user_id: String, role: String) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let valid_roles = ["user", "moderator", "admin"];
    if !valid_roles.contains(&role.as_str()) {
        return Err(ServerFnError::new(format!("Invalid role: {}", role)));
    }

    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::set_user_role(pool, &user_id, &role)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn delete_user(user_id: String) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::delete_user(pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ─── Invite Codes ─────────────────────────────────────────────────────────────

#[server]
pub async fn create_invite_code() -> Result<InviteCode, ServerFnError> {
    use rand::Rng;

    let caller = crate::server_fns::helpers::require_role("admin").await?;

    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();

    let pool = crate::db::get_auth_pool().await;
    let id = crate::db::auth::create_invite_code(pool, &code, &caller.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let invite = crate::db::auth::get_invite_code(pool, &code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new(format!("Invite code {} not found after creation", id)))?;

    Ok(invite)
}

#[server]
pub async fn list_invite_codes() -> Result<Vec<InviteCode>, ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::list_invite_codes(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn revoke_invite_code(code_id: i64) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::revoke_invite_code(pool, code_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ─── Rooms ────────────────────────────────────────────────────────────────────

#[server]
pub async fn create_room(name: String, topic: String, room_type: String) -> Result<Room, ServerFnError> {
    use rand::Rng;

    crate::server_fns::helpers::require_role("admin").await?;

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ServerFnError::new("Room name cannot be empty"));
    }

    let valid_types = ["public", "private"];
    if !valid_types.contains(&room_type.as_str()) {
        return Err(ServerFnError::new("Invalid room type"));
    }

    let topic_opt: Option<&str> = if topic.trim().is_empty() {
        None
    } else {
        Some(topic.trim())
    };

    let invite_code: Option<String> = if room_type == "private" {
        Some(
            rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(16)
                .map(char::from)
                .collect(),
        )
    } else {
        None
    };

    let caller = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_chat_pool().await;
    let room_id = crate::db::chat::create_room(pool, &name, topic_opt, &room_type, invite_code.as_deref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Auto-add creator as member and broadcast so their sidebar refreshes.
    crate::db::chat::add_room_member(pool, room_id, &caller.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    crate::ws::hub::get_hub().broadcast_to_user(
        &caller.id,
        &crate::ws::events::ChatEvent::RoomMemberAdded { room_id, user_id: caller.id.clone() },
    );

    crate::db::chat::get_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new(format!("Room {} not found after creation", room_id)))
}

#[server]
pub async fn delete_room(room_id: i64) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::delete_room(pool, room_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn update_room(room_id: i64, name: String, topic: String) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("admin").await?;

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ServerFnError::new("Room name cannot be empty"));
    }

    let topic_opt: Option<&str> = if topic.trim().is_empty() {
        None
    } else {
        Some(topic.trim())
    };

    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::update_room(pool, room_id, &name, topic_opt)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn invite_user_to_room(room_id: i64, username: String) -> Result<(), ServerFnError> {
    crate::server_fns::helpers::require_role("moderator").await?;

    let auth_pool = crate::db::get_auth_pool().await;
    let target = crate::db::auth::find_user_by_username(auth_pool, &username)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new(format!("User '{}' not found", username)))?;

    let chat_pool = crate::db::get_chat_pool().await;
    crate::db::chat::add_room_member(chat_pool, room_id, &target.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let event = crate::ws::events::ChatEvent::RoomMemberAdded {
        room_id,
        user_id: target.id.clone(),
    };
    crate::ws::hub::get_hub().broadcast_global(&event);

    Ok(())
}

#[server]
pub async fn regenerate_invite_code(room_id: i64) -> Result<String, ServerFnError> {
    use rand::Rng;

    crate::server_fns::helpers::require_role("admin").await?;

    let new_code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();

    let pool = crate::db::get_chat_pool().await;
    crate::db::chat::regenerate_invite_code(pool, room_id, &new_code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(new_code)
}
