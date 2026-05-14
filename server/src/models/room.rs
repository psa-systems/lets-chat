use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: i64,
    pub name: String,
    pub topic: Option<String>,
    /// Visibility: "public" | "private" | "dm". Independent of `is_voice`.
    pub room_type: String,
    pub invite_code: Option<String>,
    pub created_at: String,
    /// True for voice channels (vs. text channels). Orthogonal to
    /// `room_type` - a voice channel may be public or private.
    pub is_voice: bool,
}
