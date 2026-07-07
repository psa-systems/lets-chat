//! LC-546: per-thread mute. The inverse of LC-310 thread following - a muter is
//! dropped from a thread's reply fan-out even when they are an auto-followed
//! participant, so one noisy thread can be silenced without unfollowing the
//! rest. Keyed by the thread root message (`parent_id`), mirroring
//! `thread_followers`. Enforced in `routes::room::notify_thread_followers`.
use sqlx::{Row, SqlitePool};

/// Mute a thread. Idempotent (PRIMARY KEY(user_id, parent_id) + OR IGNORE).
pub async fn mute(
    pool: &SqlitePool,
    user_id: &str,
    parent_id: i64,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO thread_muters (user_id, parent_id, room_id) VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(parent_id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unmute(pool: &SqlitePool, user_id: &str, parent_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM thread_muters WHERE user_id = ? AND parent_id = ?")
        .bind(user_id)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn is_muted(
    pool: &SqlitePool,
    user_id: &str,
    parent_id: i64,
) -> Result<bool, sqlx::Error> {
    let row =
        sqlx::query("SELECT 1 AS x FROM thread_muters WHERE user_id = ? AND parent_id = ? LIMIT 1")
            .bind(user_id)
            .bind(parent_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Every muter of a thread, for filtering the reply fan-out in one query
/// instead of a per-follower round-trip.
pub async fn muters(pool: &SqlitePool, parent_id: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id FROM thread_muters WHERE parent_id = ?")
        .bind(parent_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("user_id"))
        .collect())
}
