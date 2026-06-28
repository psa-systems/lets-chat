//! LC-490: message acknowledgement / required-read tracking.
//!
//! Two tables (see migration `0073_acknowledgements.sql`): `message_ack_required`
//! is the pin-shaped "needs ack" flag, `message_acks` is the reaction-shaped
//! per-user roster. The `required_ids_for_room` bulk path mirrors
//! `pinned::pinned_message_ids_for_room`; `rollup` mirrors `chat::list_reactions`.

use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

/// Aggregate ack state for one message, as seen by a caller.
#[derive(Debug, Default, Clone)]
pub struct AckRollup {
    pub count: i64,
    pub acked_by_me: bool,
    pub acker_ids: Vec<String>,
}

/// Flag a message as needing acknowledgement (idempotent).
pub async fn set_required(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    required_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_ack_required (message_id, room_id, required_by) VALUES (?, ?, ?) \
         ON CONFLICT(message_id) DO NOTHING",
    )
    .bind(message_id)
    .bind(room_id)
    .bind(required_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the requirement and drop the roster rows in one step.
pub async fn clear_required(pool: &SqlitePool, message_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM message_ack_required WHERE message_id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM message_acks WHERE message_id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether a message currently requires acknowledgement.
pub async fn is_required(pool: &SqlitePool, message_id: i64) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM message_ack_required WHERE message_id = ?")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Set of message ids in `room_id` that require acknowledgement. Bulk page-render
/// path (mirrors `pinned::pinned_message_ids_for_room`).
pub async fn required_ids_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query("SELECT message_id FROM message_ack_required WHERE room_id = ?")
        .bind(room_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("message_id"))
        .collect())
}

/// Record `user_id`'s acknowledgement (idempotent). Returns `true` if a new row
/// was inserted (a first-time ack), `false` if it already existed.
pub async fn acknowledge(
    pool: &SqlitePool,
    message_id: i64,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO message_acks (message_id, user_id) VALUES (?, ?) \
         ON CONFLICT(message_id, user_id) DO NOTHING",
    )
    .bind(message_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Aggregate the roster for one message as seen by `caller_user_id`.
pub async fn rollup(
    pool: &SqlitePool,
    message_id: i64,
    caller_user_id: &str,
) -> Result<AckRollup, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count, \
                MAX(CASE WHEN user_id = ? THEN 1 ELSE 0 END) AS acked_by_me, \
                group_concat(user_id) AS acker_ids \
         FROM message_acks WHERE message_id = ?",
    )
    .bind(caller_user_id)
    .bind(message_id)
    .fetch_one(pool)
    .await?;
    Ok(AckRollup {
        count: row.get("count"),
        acked_by_me: row.get::<i64, _>("acked_by_me") == 1,
        acker_ids: split_ids(row.get::<Option<String>, _>("acker_ids")),
    })
}

/// Parse a `group_concat(user_id)` blob. User ids never contain commas, so a
/// plain split is safe; NULL / empty -> empty vec.
fn split_ids(raw: Option<String>) -> Vec<String> {
    match raw {
        Some(s) if !s.is_empty() => s.split(',').map(|p| p.to_string()).collect(),
        _ => Vec::new(),
    }
}
