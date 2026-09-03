//! LC-859: append-only observability log for the voice/huddle call lifecycle.
//!
//! Modeled on [`crate::db::remote_control_audit`]: `log_event` appends one row
//! from the WS voice handlers (connect / reconnect / left / dropped / mute /
//! unmute), `list_recent` feeds the admin `/admin/voice-log` view, and
//! `had_recent_departure` lets a fresh join classify itself as a reconnect. The
//! ids are auth.db user ids resolved to names at write time (denormalized into
//! `user_label`) so the log reads correctly even after a rename or delete. This
//! is a traceability record, not a content log: no SDP/ICE payloads, no message
//! text.

use sqlx::{Row, SqlitePool};

/// How long a fresh voice log is kept. A live-debug log is chatty (a mute toggle
/// is a row), so it is bounded by age rather than left to grow forever; pruning
/// runs opportunistically on each append.
const RETENTION: &str = "-1 day";

/// One row of the voice-event log, newest-first in the admin listing.
#[derive(Debug, Clone)]
pub struct VoiceEvent {
    pub room_id: i64,
    pub user_id: String,
    /// Display label captured at event time (may differ from the user's current
    /// name); empty only for rows written before a label was available.
    pub user_label: String,
    /// Fixed vocabulary: connect / reconnect / left / dropped / mute / unmute.
    pub kind: String,
    pub detail: Option<String>,
    pub created_at: String,
}

/// Append one lifecycle event, then prune rows past the retention window.
///
/// Best-effort at the call sites (a logging failure must never affect a live
/// call), so this returns the sqlx result for the caller to swallow. `kind` is a
/// fixed vocabulary the WS handlers pass as constants; no validation here.
pub async fn log_event(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    user_label: &str,
    kind: &str,
    detail: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO voice_events (room_id, user_id, user_label, kind, detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(user_label)
    .bind(kind)
    .bind(detail)
    .execute(pool)
    .await?;
    // Bound the table to the retention window. Indexed on created_at, and at
    // voice-event volume this is cheap; a failure here is non-fatal (the row is
    // already written), so it is logged by the caller's swallow, not surfaced.
    sqlx::query("DELETE FROM voice_events WHERE created_at < datetime('now', ?1)")
        .bind(RETENTION)
        .execute(pool)
        .await?;
    Ok(())
}

/// The most recent events, newest first, for the admin listing. Bounded by
/// `limit` so the page never renders an unbounded table.
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<VoiceEvent>> {
    let rows = sqlx::query(
        "SELECT room_id, user_id, user_label, kind, detail, created_at
         FROM voice_events ORDER BY created_at DESC, id DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| VoiceEvent {
            room_id: r.get("room_id"),
            user_id: r.get("user_id"),
            user_label: r.get("user_label"),
            kind: r.get("kind"),
            detail: r.get("detail"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// True when `user_id` logged a departure (left / dropped) from `room_id` within
/// the last `within_secs` seconds. A join that follows one is a reconnect - the
/// exact LC-764 "peer dropped then came back" case an admin needs to see named,
/// derived from the log itself rather than tracked in memory.
pub async fn had_recent_departure(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    within_secs: i64,
) -> sqlx::Result<bool> {
    let window = format!("-{within_secs} seconds");
    let row = sqlx::query(
        "SELECT 1 FROM voice_events
         WHERE room_id = ?1 AND user_id = ?2 AND kind IN ('left', 'dropped')
           AND created_at > datetime('now', ?3)
         LIMIT 1",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(window)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}
