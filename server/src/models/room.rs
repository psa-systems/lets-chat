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
    /// LC-86: long-form description shown on the room info page below
    /// the (short) topic. Optional; None for rooms that never set one.
    pub description: Option<String>,
    /// LC-86: single wiki / docs page body in Markdown source. Rendered
    /// through `views::markdown` on the info page. None when no wiki has
    /// been written.
    pub wiki_body: Option<String>,
    /// LC-86: timestamp of the last wiki edit (`datetime('now')`). None
    /// when no wiki has ever been written.
    pub wiki_updated_at: Option<String>,
    /// LC-86: user_id of the last wiki editor. None when no wiki has
    /// ever been written. The info page resolves this to a display label.
    pub wiki_updated_by: Option<String>,
}
