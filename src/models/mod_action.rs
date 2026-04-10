use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModAction {
    pub id: i64,
    pub action: String,
    pub target_user: String,
    pub actor_user: String,
    pub reason: Option<String>,
    pub room_id: Option<i64>,
    pub metadata: Option<String>,
    pub created_at: String,
}
