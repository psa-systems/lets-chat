use axum::http::header::{HeaderName, HeaderValue, COOKIE, SET_COOKIE};
use axum::http::HeaderMap;

pub const COOKIE_NAME: &str = "lets_chat_last_visited";

pub fn read(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

pub fn set(path: &str) -> (HeaderName, HeaderValue) {
    let v = format!("{COOKIE_NAME}={path}; Path=/; HttpOnly; Secure; SameSite=Strict");
    (SET_COOKIE, HeaderValue::from_str(&v).unwrap())
}

/// Permitted shapes: `/room/<i64>` and `/dm/<peer_id>` where peer_id is
/// alphanumeric / hyphen only (UUIDs are the format produced by the auth DB).
pub fn is_safe_path(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("/room/") {
        return !rest.is_empty() && rest.parse::<i64>().is_ok();
    }
    if let Some(rest) = path.strip_prefix("/dm/") {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths_pass() {
        assert!(is_safe_path("/room/1"));
        assert!(is_safe_path("/room/12345"));
        assert!(is_safe_path("/dm/550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn unsafe_paths_rejected() {
        assert!(!is_safe_path("/"));
        assert!(!is_safe_path("/room/"));
        assert!(!is_safe_path("/room/abc"));
        assert!(!is_safe_path("/dm/"));
        assert!(!is_safe_path("/dm/../etc/passwd"));
        assert!(!is_safe_path("/admin"));
        assert!(!is_safe_path("//evil.example/"));
    }
}
