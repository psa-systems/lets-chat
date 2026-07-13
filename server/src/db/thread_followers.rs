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

/// LC-575: one row per followed thread that has replies the viewer has not
/// caught up on, for the Home dashboard "Threads" card.
pub struct FollowedThreadDigest {
    pub parent_id: i64,
    pub room_id: i64,
    pub room_name: String,
    /// Root message body, for the row preview.
    pub parent_preview: String,
    pub unread_replies: i64,
}

/// LC-575: followed threads carrying replies newer than the viewer's read
/// watermark. There is no per-thread watermark, so this reuses the room-level
/// `dm_read_state` last-read id the sidebar unread counts already use: a reply
/// counts when its id is past that watermark and it is not the viewer's own.
/// Newest-activity first, capped so the card stays a glance.
pub async fn followed_threads_with_unread(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<FollowedThreadDigest>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT tf.parent_id AS parent_id, \
                tf.room_id AS room_id, \
                r.name AS room_name, \
                p.body AS parent_preview, \
                COUNT(m.id) AS unread_replies \
           FROM thread_followers tf \
           JOIN rooms r ON r.id = tf.room_id \
           JOIN messages p ON p.id = tf.parent_id AND p.deleted_at IS NULL \
           LEFT JOIN dm_read_state s ON s.room_id = tf.room_id AND s.user_id = ? \
           JOIN messages m ON m.parent_id = tf.parent_id \
            AND m.user_id != ? \
            AND m.deleted_at IS NULL AND m.quarantined = 0 \
            AND m.id > COALESCE(s.last_read_message_id, 0) \
          WHERE tf.user_id = ? \
          GROUP BY tf.parent_id, tf.room_id, r.name, p.body \
          HAVING unread_replies > 0 \
          ORDER BY MAX(m.id) DESC \
          LIMIT 20",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| FollowedThreadDigest {
            parent_id: r.get("parent_id"),
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            parent_preview: r.get("parent_preview"),
            unread_replies: r.get::<i64, _>("unread_replies"),
        })
        .collect())
}
