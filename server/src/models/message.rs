use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// `Some(N)` when this message is a reply in the thread rooted at `N`.
    pub parent_id: Option<i64>,
}
