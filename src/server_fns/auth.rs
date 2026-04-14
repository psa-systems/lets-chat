use dioxus::prelude::*;

use crate::models::User;

/// Response from login/register that includes a session token for the client to store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthResponse {
    pub user: User,
    pub session_token: String,
}

pub const GENERIC_REGISTER_ERROR: &str = "Registration failed";
pub const GENERIC_LOGIN_ERROR: &str = "Invalid credentials";
pub const TRY_AGAIN_ERROR: &str = "Something went wrong, please try again";

/// Length of the hydration-race grace window on the register/login pages.
/// Blank submissions within this window of page mount show TRY_AGAIN_ERROR;
/// after, they show the generic error.
pub const GRACE_WINDOW_MS: f64 = 120_000.0;

/// Current wall-clock time in milliseconds. Cross-platform: `js_sys::Date::now`
/// on wasm, `SystemTime` elsewhere.
pub fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    }
}

/// Extract the user-facing message from a `ServerFnError`. The default `Display`
/// impl wraps the message as `"error running server function: <msg> (details: ...)"`,
/// which is leaky for end users.
pub fn user_facing_error(err: &ServerFnError) -> String {
    match err {
        ServerFnError::ServerError { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

#[server]
pub async fn register(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    use rand::rngs::OsRng;
    use tracing::warn;

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    if username.is_empty() || password.is_empty() {
        warn!(
            username_empty = username.is_empty(),
            password_empty = password.is_empty(),
            "register rejected: blank input"
        );
        return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
    }

    if username.len() < 3 {
        warn!(username_len = username.len(), "register rejected: username too short");
        return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
    }
    if password.len() < 8 {
        warn!(password_len = password.len(), "register rejected: password too short");
        return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
    }

    let pool = crate::db::get_auth_pool().await;

    match crate::db::auth::find_user_by_username(pool, &username).await {
        Ok(Some(_)) => {
            warn!(%username, "register rejected: username already taken");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
        Ok(None) => {}
        Err(e) => {
            warn!(error = %e, "register failed: db lookup");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = match argon2.hash_password(password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            warn!(error = %e, "register failed: password hash");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let user_id = match crate::db::auth::create_user(pool, &username, &password_hash).await {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "register failed: create_user");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let user_count = match crate::db::auth::count_users(pool).await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "register failed: count_users");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };
    if user_count == 1 {
        if let Err(e) = crate::db::auth::set_user_role(pool, &user_id, "admin").await {
            warn!(error = %e, "register failed: set_user_role admin");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    }

    let session_token = match crate::db::auth::create_session(pool, &user_id).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "register failed: create_session");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    let record = match crate::db::auth::find_user_by_id(pool, &user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!(%user_id, "register failed: user not found after creation");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
        Err(e) => {
            warn!(error = %e, "register failed: find_user_by_id");
            return Err(ServerFnError::new(GENERIC_REGISTER_ERROR));
        }
    };

    Ok(AuthResponse {
        user: user_record_to_user(&record),
        session_token,
    })
}

#[server]
pub async fn login(username: String, password: String) -> Result<AuthResponse, ServerFnError> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    use tracing::warn;

    let username = username.trim().to_string();
    let password = password.trim().to_string();

    if username.is_empty() || password.is_empty() {
        warn!(
            username_empty = username.is_empty(),
            password_empty = password.is_empty(),
            "login rejected: blank input"
        );
        return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
    }

    let pool = crate::db::get_auth_pool().await;

    let record = match crate::db::auth::find_user_by_username(pool, &username).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            warn!(%username, "login rejected: user not found");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
        Err(e) => {
            warn!(error = %e, "login failed: db lookup");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };

    let parsed_hash = match PasswordHash::new(&record.password_hash) {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "login failed: stored hash unparseable");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_err()
    {
        warn!(%username, "login rejected: bad password");
        return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
    }

    // Ban status is preserved as a distinct, user-facing message per design.
    if record.is_banned {
        let msg = match &record.ban_reason {
            Some(reason) => format!("Account banned: {}", reason),
            None => "Account banned".to_string(),
        };
        return Err(ServerFnError::new(msg));
    }

    let session_token = match crate::db::auth::create_session(pool, &record.id).await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "login failed: create_session");
            return Err(ServerFnError::new(GENERIC_LOGIN_ERROR));
        }
    };

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
