use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteCode {
    pub id: i64,
    pub code: String,
    pub created_by: String,
    pub used_by: Option<String>,
    pub used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}
