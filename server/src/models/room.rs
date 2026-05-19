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
    /// LC-85: who is allowed to post in this room. One of `"all"`,
    /// `"moderators_only"`, `"admins_only"`. Compose-box rendering and
    /// the `post_message` handler both consult this; reactions / pins
    /// / edits-of-own-messages do not. New rooms default to `"all"` via
    /// the migration's column DEFAULT.
    pub posting_allowed_for: String,
}
