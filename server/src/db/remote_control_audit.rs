//! LC-186: audit trail for remote-control sessions (LC-181).
//!
//! A session row is opened when the sharer grants control and closed when
//! control ends. Writes are driven from [`crate::routes::ws::relay_control_signal`]
//! (open on `grant`, close on `revoke`) with a socket-disconnect backstop that
//! closes any session a hard drop left open. The table stores only participant
//! ids + timestamps - no message content - so it is a traceability record, not
//! a content log.

use sqlx::SqlitePool;

/// Open a session row for a freshly-granted control session. No-op if a row is
/// already open for the room, so a repeated `grant` (e.g. a re-grant after a
/// transient revoke) does not stack duplicate open rows.
pub async fn start_session(
    pool: &SqlitePool,
    room_id: i64,
    controller_id: &str,
    sharer_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO remote_control_sessions (room_id, controller_id, sharer_id)
         SELECT ?1, ?2, ?3
         WHERE NOT EXISTS (
             SELECT 1 FROM remote_control_sessions
             WHERE room_id = ?1 AND ended_at IS NULL
         )",
    )
    .bind(room_id)
    .bind(controller_id)
    .bind(sharer_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Close the open session for a room (kill-switch / revoke / auto-revoke).
/// No-op if none is open.
pub async fn end_session_by_room(pool: &SqlitePool, room_id: i64, reason: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?2
         WHERE room_id = ?1 AND ended_at IS NULL",
    )
    .bind(room_id)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Close every open session a user participates in (controller or sharer).
/// The socket-disconnect backstop: a hard WS drop never sends a `revoke`, so
/// without this an interrupted session would stay open forever.
pub async fn end_sessions_for_user(
    pool: &SqlitePool,
    user_id: &str,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?2
         WHERE ended_at IS NULL AND (controller_id = ?1 OR sharer_id = ?1)",
    )
    .bind(user_id)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}
