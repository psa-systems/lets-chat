use serde::{Deserialize, Serialize};

/// LC-334: a member-filed report on a message. Persisted in `chat.db`
/// (`message_reports`). `reporter_id` / `handled_by` are auth-db user ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageReport {
    pub id: i64,
    pub message_id: i64,
    pub room_id: i64,
    pub reporter_id: String,
    /// One of the `REPORT_CATEGORIES` allowlist (`spam` | `harassment` |
    /// `inappropriate` | `other`).
    pub category: String,
    pub note: Option<String>,
    /// `open` | `resolved` | `dismissed`.
    pub status: String,
    pub handled_by: Option<String>,
    pub created_at: String,
    pub handled_at: Option<String>,
}
