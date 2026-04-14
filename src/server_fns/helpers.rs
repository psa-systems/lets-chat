use dioxus::prelude::*;

use crate::models::user::UserRecord;

/// Extract the authenticated user from the session cookie.
/// Returns ServerFnError if not authenticated or banned.
pub async fn require_auth() -> Result<UserRecord, ServerFnError> {
    let session_id = get_session_id()
        .await?
        .ok_or_else(|| ServerFnError::new("Not authenticated"))?;

    let pool = crate::db::get_auth_pool().await;
    let record = crate::db::auth::get_user_by_session(pool, &session_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Session expired or invalid"))?;

    // Check ban status
    if record.is_banned {
        if let Some(ref until) = record.banned_until {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            if until.as_str() > now.as_str() {
                return Err(ServerFnError::new(format!(
                    "Account suspended until {}",
                    until
                )));
            }
        } else {
            return Err(ServerFnError::new("Account banned"));
        }
    }

    Ok(record)
}

/// Require the user has a minimum role level.
/// Role hierarchy: admin (3) > moderator (2) > user (1)
pub async fn require_role(minimum: &str) -> Result<UserRecord, ServerFnError> {
    let user = require_auth().await?;

    let user_level = role_level(&user.role);
    let required_level = role_level(minimum);

    if user_level < required_level {
        return Err(ServerFnError::new("Insufficient permissions"));
    }

    Ok(user)
}

pub fn role_level(role: &str) -> u8 {
    match role {
        "admin" => 3,
        "moderator" => 2,
        "user" => 1,
        _ => 0,
    }
}

/// Extract session ID from cookie header. Returns None if no session cookie.
pub async fn get_session_id() -> Result<Option<String>, ServerFnError> {
    let headers: http::HeaderMap = dioxus_fullstack::FullstackContext::extract().await?;
    let cookie_header = match headers.get(http::header::COOKIE) {
        Some(val) => val.to_str().unwrap_or("").to_string(),
        None => return Ok(None),
    };

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
