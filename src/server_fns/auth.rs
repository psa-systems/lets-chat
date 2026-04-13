use dioxus::prelude::*;

use crate::models::User;

/// Response from login/register that includes a session token for the client to store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthResponse {
    pub user: User,
    pub session_token: String,
}

#[server]
pub async fn register(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    use rand::rngs::OsRng;

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    if username.len() < 3 {
        return Err(ServerFnError::new(
            "Username must be at least 3 characters",
        ));
    }
    if password.len() < 8 {
        return Err(ServerFnError::new(
            "Password must be at least 8 characters",
        ));
    }

    let pool = crate::db::get_auth_pool().await;

    // Check if username already taken
    if crate::db::auth::find_user_by_username(pool, &username)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some()
    {
        return Err(ServerFnError::new("Username already taken"));
    }

    // Hash password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| ServerFnError::new(format!("Failed to hash password: {}", e)))?
        .to_string();

    // Create user
    let user_id = crate::db::auth::create_user(pool, &username, &password_hash)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Auto-promote first user to admin
    let user_count = crate::db::auth::count_users(pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if user_count == 1 {
        crate::db::auth::set_user_role(pool, &user_id, "admin")
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }

    // Create session
    let session_token = crate::db::auth::create_session(pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Fetch the user record to return public info
    let record = crate::db::auth::find_user_by_id(pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found after creation"))?;

    Ok(AuthResponse {
        user: user_record_to_user(&record),
        session_token,
    })
}

#[server]
pub async fn login(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    let pool = crate::db::get_auth_pool().await;

    let record = crate::db::auth::find_user_by_username(pool, &username)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Invalid username or password"))?;

    // Verify password
    let parsed_hash = PasswordHash::new(&record.password_hash)
        .map_err(|e| ServerFnError::new(format!("Hash error: {}", e)))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| ServerFnError::new("Invalid username or password"))?;

    // Check ban status
    if record.is_banned {
        let msg = match &record.ban_reason {
            Some(reason) => format!("Account banned: {}", reason),
            None => "Account banned".to_string(),
        };
        return Err(ServerFnError::new(msg));
    }

    // Create session
    let session_token = crate::db::auth::create_session(pool, &record.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(AuthResponse {
        user: user_record_to_user(&record),
        session_token,
    })
}

#[server]
pub async fn logout() -> Result<(), ServerFnError> {
    let pool = crate::db::get_auth_pool().await;

    // Read session from cookie
    if let Some(session_id) = get_session_from_cookie().await? {
        crate::db::auth::delete_session(pool, &session_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }

    Ok(())
}

#[server]
pub async fn get_current_user() -> Result<Option<User>, ServerFnError> {
    let pool = crate::db::get_auth_pool().await;

    let session_id = match get_session_from_cookie().await? {
        Some(id) => id,
        None => return Ok(None),
    };

    let record = crate::db::auth::get_user_by_session(pool, &session_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(record.map(|r| user_record_to_user(&r)))
}

#[cfg(feature = "server")]
fn user_record_to_user(record: &crate::models::user::UserRecord) -> User {
    User {
        id: record.id.clone(),
        username: record.username.clone(),
        display_name: record.display_name.clone(),
        role: record.role.clone(),
        is_muted: record.is_muted,
        muted_until: record.muted_until.clone(),
        is_banned: record.is_banned,
        ban_reason: record.ban_reason.clone(),
        banned_until: record.banned_until.clone(),
        created_at: record.created_at.clone(),
        read_receipts_enabled: record.read_receipts_enabled,
    }
}

#[cfg(feature = "server")]
async fn get_session_from_cookie() -> Result<Option<String>, ServerFnError> {
    crate::server_fns::helpers::get_session_id().await
}

/// Set the session cookie in the browser using document.cookie.
pub fn set_session_cookie(token: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(window) = web_sys::window() {
            if let Some(document) = window.document() {
                let document: web_sys::HtmlDocument = document.unchecked_into();
                let cookie = format!(
                    "session={}; path=/; max-age={}; SameSite=Lax",
                    token,
                    30 * 24 * 60 * 60 // 30 days
                );
                let _ = document.set_cookie(&cookie);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = token;
}

#[server]
pub async fn set_read_receipts_enabled(enabled: bool) -> Result<(), ServerFnError> {
    let me = crate::server_fns::helpers::require_auth().await?;
    let pool = crate::db::get_auth_pool().await;
    crate::db::auth::set_read_receipts_enabled(pool, &me.id, enabled)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Clear the session cookie in the browser.
pub fn clear_session_cookie() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(window) = web_sys::window() {
            if let Some(document) = window.document() {
                let document: web_sys::HtmlDocument = document.unchecked_into();
                let _ = document.set_cookie("session=; path=/; max-age=0");
            }
        }
    }
}
