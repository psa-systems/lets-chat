//! LC-1113 Phase 1: group the voice-event log into calls.
//!
//! A call is one span a voice room is non-empty. The WS voice handlers call
//! [`open`] when the room goes 0 -> 1 participant and [`close`] when it goes
//! 1 -> 0; [`crate::routes::ws::record_voice_event`] stamps each event with the
//! room's currently-open call, which each call site supplies (via [`open_id`]
//! when it has no cheaper way to get it; LC-1122). The admin `/admin/voice-log`
//! view reads [`list_recent`] (one row per call with counts) and drills into one
//! call with [`list_events`]. Like [`crate::db::voice_events`] this is a
//! traceability record, bounded by age, never a content log.

use crate::db::voice_events::VoiceEvent;
use sqlx::{Row, SqlitePool};

/// Match the voice_events retention so a call and its events age out together.
const RETENTION: &str = "-1 day";

/// One call in the admin "recent calls" listing: the span plus the counts an
/// admin scans to spot a bad call (how many people, how many error/drop events).
#[derive(Debug, Clone)]
pub struct CallSummary {
    pub id: i64,
    pub room_id: i64,
    pub started_at: String,
    /// NULL while the call is still live.
    pub ended_at: Option<String>,
    pub participants: i64,
    pub events: i64,
    /// `error` + `dropped` rows - the audio-fault signal.
    pub errors: i64,
}

/// Open a fresh call for `room_id` and return its id. Closes any stale still-open
/// call for the room first (a crash could leave one), so the one-open-per-room
/// invariant always holds and the new span is distinct. Prunes aged-out calls
/// opportunistically. Best-effort at the call site.
pub async fn open(pool: &SqlitePool, room_id: i64) -> sqlx::Result<i64> {
    sqlx::query(
        "UPDATE voice_calls SET ended_at = datetime('now') WHERE room_id = ?1 AND ended_at IS NULL",
    )
    .bind(room_id)
    .execute(pool)
    .await?;
    let id: i64 = sqlx::query("INSERT INTO voice_calls (room_id) VALUES (?1) RETURNING id")
        .bind(room_id)
        .fetch_one(pool)
        .await?
        .get("id");
    sqlx::query(
        "DELETE FROM voice_calls WHERE ended_at IS NOT NULL AND started_at < datetime('now', ?1)",
    )
    .bind(RETENTION)
    .execute(pool)
    .await?;
    Ok(id)
}

/// The id of `room_id`'s currently-open call, or None. Used by
/// `record_voice_event` to stamp an event with its call.
pub async fn open_id(pool: &SqlitePool, room_id: i64) -> sqlx::Result<Option<i64>> {
    let row = sqlx::query("SELECT id FROM voice_calls WHERE room_id = ?1 AND ended_at IS NULL")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("id")))
}

/// Close `room_id`'s open call (the last participant left). Idempotent: a no-op
/// when nothing is open.
pub async fn close(pool: &SqlitePool, room_id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE voice_calls SET ended_at = datetime('now') WHERE room_id = ?1 AND ended_at IS NULL",
    )
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Recent calls, newest first, each with its participant and event/error counts.
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<CallSummary>> {
    let rows = sqlx::query(
        "SELECT c.id, c.room_id, c.started_at, c.ended_at,
                COUNT(DISTINCT e.user_id) AS participants,
                COUNT(e.id) AS events,
                COALESCE(SUM(CASE WHEN e.kind IN ('error', 'dropped') THEN 1 ELSE 0 END), 0) AS errors
         FROM voice_calls c
         LEFT JOIN voice_events e ON e.call_id = c.id
         GROUP BY c.id
         ORDER BY c.started_at DESC, c.id DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_summary).collect())
}

/// One call's header for the drill-down, or None if it aged out / never existed.
pub async fn get(pool: &SqlitePool, call_id: i64) -> sqlx::Result<Option<CallSummary>> {
    let row = sqlx::query(
        "SELECT c.id, c.room_id, c.started_at, c.ended_at,
                COUNT(DISTINCT e.user_id) AS participants,
                COUNT(e.id) AS events,
                COALESCE(SUM(CASE WHEN e.kind IN ('error', 'dropped') THEN 1 ELSE 0 END), 0) AS errors
         FROM voice_calls c
         LEFT JOIN voice_events e ON e.call_id = c.id
         WHERE c.id = ?1
         GROUP BY c.id",
    )
    .bind(call_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_summary))
}

/// One call's events, oldest first, for the drill-down timeline.
pub async fn list_events(pool: &SqlitePool, call_id: i64) -> sqlx::Result<Vec<VoiceEvent>> {
    let rows = sqlx::query(
        "SELECT room_id, user_id, user_label, kind, detail, created_at
         FROM voice_events WHERE call_id = ?1 ORDER BY created_at ASC, id ASC",
    )
    .bind(call_id)
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

fn row_to_summary(r: sqlx::sqlite::SqliteRow) -> CallSummary {
    CallSummary {
        id: r.get("id"),
        room_id: r.get("room_id"),
        started_at: r.get("started_at"),
        ended_at: r.get("ended_at"),
        participants: r.get("participants"),
        events: r.get("events"),
        errors: r.get("errors"),
    }
}
