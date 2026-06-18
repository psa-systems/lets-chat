use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnclaveRole {
    Owner,
    Admin,
    Member,
}

impl EnclaveRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnclaveRole::Owner => "owner",
            EnclaveRole::Admin => "admin",
            EnclaveRole::Member => "member",
        }
    }
}

impl FromStr for EnclaveRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(EnclaveRole::Owner),
            "admin" => Ok(EnclaveRole::Admin),
            "member" => Ok(EnclaveRole::Member),
            other => Err(format!("invalid enclave role: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enclave {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub invite_code: Option<String>,
    pub created_by: String,
    pub created_at: String,
    /// When true, this enclave's custom emojis resolve in every other room
    /// (other enclaves and DMs). A room's own emojis still win on shortcode
    /// collisions; sharing only expands the universe of resolvable tokens.
    pub share_emojis_globally: bool,
    /// LC-217: per-enclave message send rate limit (burst per minute,
    /// shared 60 s window with the global limit). `0` means "use the
    /// global `rate_limit_messages` setting". A non-zero value chains
    /// IN ADDITION to the global cap; the per-enclave cap never relaxes
    /// the global, only tightens it for posts inside this enclave.
    pub msg_rate_limit_burst: u32,
    /// LC-339: "Coyote Mode" anti-spam. When true, a member who posts in 3+
    /// distinct rooms of this enclave within <=3 seconds is treated as a bot:
    /// banned from the enclave (kick + ban-list) and their last-24h messages
    /// in the enclave's rooms are soft-deleted. Default false.
    pub coyote_mode: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnclaveMembership {
    pub enclave_id: i64,
    pub user_id: String,
    pub role: EnclaveRole,
    pub joined_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnclaveInvitation {
    pub id: i64,
    pub enclave_id: i64,
    pub invitee_id: String,
    pub invited_by: String,
    pub created_at: String,
}
