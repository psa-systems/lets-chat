use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub message_id: i64,
    pub room_id: i64,
    pub room_name: String,
    pub body: String,
    pub user_id: String,
    pub author_name: String,
    pub created_at: String,
}
