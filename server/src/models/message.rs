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
    /// `Some(N)` when this message visually quotes the message with id `N`
    /// inline above its body. Distinct from `parent_id`: quoted messages
    /// still appear in the main timeline rather than being collapsed into
    /// a thread side panel. Always `None` for thread replies (the quote-
    /// reply affordance is suppressed inside the thread panel).
    pub quote_id: Option<i64>,
    /// True for server-authored system notices (e.g. "started a call"),
    /// which render as a centered, non-interactive line.
    pub is_system: bool,
}
