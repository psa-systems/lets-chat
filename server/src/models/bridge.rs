use serde::{Deserialize, Serialize};

/// LC-78: a registered protocol bridge. The bridge daemon (out of process,
/// operator-run) translates between lets-chat and a foreign protocol
/// (`matrix` in v1; `irc` / `xmpp` are pure daemon-side follow-ups). The
/// server stores the registration, sealed config, and last-heartbeat health;
/// the daemon authenticates as `bot_user_id` with `bridge:post` /
/// `bridge:heartbeat` scopes (LC-72 / LC-73).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    pub id: i64,
    pub room_id: i64,
    /// Protocol kind. `matrix` in v1; column is plain TEXT in SQLite (no
    /// CHECK), so adding `irc` / `xmpp` is a code-level validation loosening,
    /// not a migration.
    pub kind: String,
    /// Bot user (LC-73) the daemon authenticates as. TEXT (no FK) because
    /// users live in auth.db; this points across the database boundary.
    pub bot_user_id: String,
    /// `pending` (just registered, daemon not yet seen) -> `healthy`
    /// (heartbeat within threshold) -> `stale` (no heartbeat for
    /// 3x interval) -> `errored` (daemon reported a fault).
    pub status: String,
    pub last_heartbeat_at: Option<String>,
    pub last_error: Option<String>,
    pub created_by: String,
    pub created_at: String,
}
