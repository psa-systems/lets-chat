use dioxus::prelude::*;

use crate::models::User;

#[server]
pub async fn register(username: String, password: String) -> Result<User, ServerFnError> {
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

    // Set session cookie
    set_session_cookie(&session_token)?;

    // Fetch the user record to return public info
    let record = crate::db::auth::find_user_by_id(pool, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found after creation"))?;

    Ok(user_record_to_user(&record))
}

#[server]
pub async fn login(username: String, password: String) -> Result<User, ServerFnError> {
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

    // Set session cookie
    set_session_cookie(&session_token)?;

    Ok(user_record_to_user(&record))
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

    // Clear cookie
    clear_session_cookie()?;

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
        created_at: record.created_at.clone(),
    }
}

#[cfg(feature = "server")]
fn set_session_cookie(token: &str) -> Result<(), ServerFnError> {
    use axum_extra::extract::cookie::Cookie;
    use time::Duration;

    let cookie = Cookie::build(("session", token.to_string()))
        .path("/")
        .http_only(true)
        .secure(false) // set to true in production with HTTPS
        .max_age(Duration::days(30))
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build();

    server_context()
        .response_headers_mut()
        .append(
            http::header::SET_COOKIE,
            cookie.to_string().parse().map_err(|e: http::header::InvalidHeaderValue| {
                ServerFnError::new(format!("Cookie header error: {}", e))
            })?,
        );

    Ok(())
}

#[cfg(feature = "server")]
fn clear_session_cookie() -> Result<(), ServerFnError> {
    use axum_extra::extract::cookie::Cookie;
    use time::Duration;

    let cookie = Cookie::build(("session", ""))
        .path("/")
        .http_only(true)
        .max_age(Duration::seconds(0))
        .build();

    server_context()
        .response_headers_mut()
        .append(
            http::header::SET_COOKIE,
            cookie.to_string().parse().map_err(|e: http::header::InvalidHeaderValue| {
                ServerFnError::new(format!("Cookie header error: {}", e))
            })?,
        );

    Ok(())
}

#[cfg(feature = "server")]
async fn get_session_from_cookie() -> Result<Option<String>, ServerFnError> {
    let headers: http::HeaderMap = extract().await?;
    let cookie_header = match headers.get(http::header::COOKIE) {
        Some(val) => val.to_str().unwrap_or("").to_string(),
        None => return Ok(None),
    };

    // Parse "session=xxx" from cookie header
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("session=") {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(Some(value.to_string()));
            }
        }
    }

    Ok(None)
}
