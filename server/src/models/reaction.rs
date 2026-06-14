use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reaction {
    pub emoji: String,
    pub count: i64,
    pub reacted_by_me: bool,
    /// LC-266: user ids of everyone who reacted with this emoji, for the
    /// "who reacted" tooltip. Resolved to display names at render time.
    #[serde(default)]
    pub reactor_ids: Vec<String>,
}
