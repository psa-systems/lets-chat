use serde::{Deserialize, Serialize};

/// Full user record. Only used server-side - never sent to the client.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: String,
    pub role: String,
    pub is_banned: bool,
    pub ban_reason: Option<String>,
    pub banned_until: Option<String>,
    pub is_muted: bool,
    pub muted_until: Option<String>,
    pub mute_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub read_receipts_enabled: bool,
}

/// Public user info safe to send to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub is_muted: bool,
    pub muted_until: Option<String>,
    pub is_banned: bool,
    pub ban_reason: Option<String>,
    pub banned_until: Option<String>,
    pub created_at: String,
    pub read_receipts_enabled: bool,
}

impl User {
    /// Construct a User without DB access.
    ///
    /// This is a temporary helper used during the rewrite so the homepage can
    /// render before real auth lands. Removed in Task 4 once `require_auth`
    /// is wired up.
    pub fn placeholder() -> Self {
        Self {
            id: "anonymous".to_string(),
            username: "anonymous".to_string(),
            display_name: None,
            role: "user".to_string(),
            is_muted: false,
            muted_until: None,
            is_banned: false,
            ban_reason: None,
            banned_until: None,
            created_at: String::new(),
            read_receipts_enabled: false,
        }
    }
}
