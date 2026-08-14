use serde::{Deserialize, Serialize};

/// LC-714: a support ticket filed by the AI help desk when a user asked for a
/// human (`/human`) and no admin was available. Persisted in `chat.db`
/// (`support_tickets`). `requester_id` / `handled_by` are auth-db user ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupportTicket {
    pub id: i64,
    pub requester_id: String,
    /// Origin conversation the request came from (the room `/human` was run in).
    /// Nullable: the conversation may be gone by the time the ticket is handled.
    pub room_id: Option<i64>,
    /// Denormalized room name at file time, for the queue's jump link/label.
    pub room_name: String,
    /// The user's request text (their `/human` note).
    pub body: String,
    /// `open` | `resolved`.
    pub status: String,
    pub handled_by: Option<String>,
    pub created_at: String,
    pub handled_at: Option<String>,
}
