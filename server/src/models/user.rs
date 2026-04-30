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

