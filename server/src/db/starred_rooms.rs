//! Per-user starred (favorited) rooms (LC-80).
//!
//! Stars are private: each user maintains their own list and ordering.
//! Lives in auth.db. The `room_id` references a chat.db rooms row; the
//! cross-db FK can't be enforced by SQLite, so [`forget_room`] runs on
//! room-leave to scrub orphans.
use std::collections::{HashMap, HashSet};

use sqlx::{Row, SqlitePool};

/// Insert a star row for `(user_id, room_id)`. Position defaults to
/// one past the user's current max so a newly-starred room lands at
/// the bottom of the Starred section. Idempotent: re-starring an
/// already-starred room is a successful no-op.
pub async fn star(pool: &SqlitePool, user_id: &str, room_id: i64) -> Result<(), sqlx::Error> {
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM starred_rooms WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO starred_rooms (user_id, room_id, position) VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(next_position)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove the star row for `(user_id, room_id)`. Idempotent.
pub async fn unstar(pool: &SqlitePool, user_id: &str, room_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM starred_rooms WHERE user_id = ? AND room_id = ?")
        .bind(user_id)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// True when the room is currently starred by the user. Used by the
/// route handler post-toggle to render the star icon in its new state.
pub async fn is_starred(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let n: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM starred_rooms WHERE user_id = ? AND room_id = ?")
            .bind(user_id)
            .bind(room_id)
            .fetch_optional(pool)
            .await?;
    Ok(n.is_some())
}

/// Set of starred room ids for the user. The sidebar loader bucketizes
/// against this in O(rooms) so a star check per row stays cheap.
pub async fn starred_room_ids(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query("SELECT room_id FROM starred_rooms WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("room_id"))
        .collect())
}

/// Per-user `room_id -> position` map so the sidebar can sort starred
/// rooms by user-controlled order without an extra query per row.
pub async fn star_positions(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashMap<i64, i64>, sqlx::Error> {
    let rows = sqlx::query("SELECT room_id, position FROM starred_rooms WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<i64, _>("room_id"), r.get::<i64, _>("position")))
        .collect())
}

/// Apply a new ordering for the user's starred rooms. Rooms not in the
/// list keep their existing position; this keeps a partial reorder
/// (eg. only the visible window) from clobbering the user's offscreen
/// state.
pub async fn set_positions(
    pool: &SqlitePool,
    user_id: &str,
    room_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (idx, room_id) in room_ids.iter().enumerate() {
        sqlx::query("UPDATE starred_rooms SET position = ? WHERE user_id = ? AND room_id = ?")
            .bind(idx as i64)
            .bind(user_id)
            .bind(room_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// AC #4: leaving a room un-stars it for the leaving user. Called from
/// the enclave-leave / kick / room-removal handlers.
pub async fn forget_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    unstar(pool, user_id, room_id).await
}

/// Bulk variant for enclave-leave / enclave-kick: drops every star
/// against any of the given room ids in one statement.
pub async fn forget_rooms(
    pool: &SqlitePool,
    user_id: &str,
    room_ids: &[i64],
) -> Result<(), sqlx::Error> {
    if room_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", room_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("DELETE FROM starred_rooms WHERE user_id = ? AND room_id IN ({placeholders})");
    let mut q = sqlx::query(&sql).bind(user_id);
    for id in room_ids {
        q = q.bind(id);
    }
    q.execute(pool).await?;
    Ok(())
}
