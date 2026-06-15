//! LC-310: thread following. Subscriptions keyed by the thread root message
//! (`parent_id`). Followers are notified of new replies via the mention
//! fan-out (no mentions rows) - see `routes::room::post_thread_reply`.
use sqlx::{Row, SqlitePool};

/// Follow a thread. Idempotent (PRIMARY KEY(user_id, parent_id) + OR IGNORE),
/// so auto-following a re-replier or an already-following user is a no-op.
pub async fn follow(
    pool: &SqlitePool,
    user_id: &str,
    parent_id: i64,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO thread_followers (user_id, parent_id, room_id) VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(parent_id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unfollow(pool: &SqlitePool, user_id: &str, parent_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM thread_followers WHERE user_id = ? AND parent_id = ?")
        .bind(user_id)
        .bind(parent_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn is_following(
    pool: &SqlitePool,
    user_id: &str,
    parent_id: i64,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT 1 AS x FROM thread_followers WHERE user_id = ? AND parent_id = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(parent_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Every follower of a thread, for fan-out on a new reply.
pub async fn followers(pool: &SqlitePool, parent_id: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id FROM thread_followers WHERE parent_id = ?")
        .bind(parent_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("user_id"))
        .collect())
}
