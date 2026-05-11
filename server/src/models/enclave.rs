use serde::{Deserialize, Serialize};

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
    pub fn from_str(s: &str) -> Result<Self, String> {
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
