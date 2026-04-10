use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub room_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}
