//! Per-user saved-messages list.
//!
//! Mirrors the pin/unpin shape but is scoped to a single user: a bookmark
//! row is only visible to the user who created it. The route layer ensures
//! the user is allowed to see the message before inserting a row.
use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

/// One bookmark row resolved against the underlying message, room, and
/// author. The Saved-page query fans out these joins in a single SQL
/// statement so the view layer does not have to do per-row lookups.
pub struct BookmarkRow {
    pub message_id: i64,
    pub created_at: String,
    pub message_body: String,
    pub message_created_at: String,
    pub author_user_id: String,
    pub room_id: i64,
    pub room_name: String,
    pub room_type: String,
}

/// Insert a bookmark for `(user_id, message_id)`. Idempotent: a second
/// bookmark for the same pair is a successful no-op (relies on the
/// composite primary key and `OR IGNORE`).
pub async fn bookmark_message(
    pool: &SqlitePool,
    user_id: &str,
    message_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO bookmarks (user_id, message_id) VALUES (?, ?)")
        .bind(user_id)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a bookmark for `(user_id, message_id)`. Idempotent.
pub async fn unbookmark_message(
    pool: &SqlitePool,
    user_id: &str,
    message_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM bookmarks WHERE user_id = ? AND message_id = ?")
        .bind(user_id)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Is `message_id` currently bookmarked by `user_id`? Used by the route
/// handler post-mutation to render the Save/Unsave button in its new state.
pub async fn is_bookmarked(
    pool: &SqlitePool,
    user_id: &str,
    message_id: i64,
) -> Result<bool, sqlx::Error> {
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM bookmarks WHERE user_id = ? AND message_id = ?")
            .bind(user_id)
            .bind(message_id)
            .fetch_optional(pool)
            .await?;
    Ok(exists.is_some())
}

/// Set of message ids currently bookmarked by `user_id` within `room_id`.
/// The page render path uses this to populate `MessageView::is_bookmarked`
/// in bulk so the room view doesn't fall to N+1 lookups.
pub async fn bookmarked_message_ids_in_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT b.message_id FROM bookmarks b \
           INNER JOIN messages m ON m.id = b.message_id \
          WHERE b.user_id = ? AND m.room_id = ? AND m.deleted_at IS NULL AND m.quarantined = 0",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("message_id"))
        .collect())
}

/// Newest-first list of bookmarks for `user_id`. Filters out rows whose
/// underlying message has been soft-deleted so the Saved page never shows
/// a tombstone. Joined to `rooms` so the view can build the "in #room" or
/// "with @peer" caption for each bookmark.
pub async fn bookmarks_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<BookmarkRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT b.message_id, b.created_at, \
                m.body AS message_body, m.created_at AS message_created_at, \
                m.user_id AS author_user_id, \
                r.id AS room_id, r.name AS room_name, r.room_type AS room_type \
           FROM bookmarks b \
           INNER JOIN messages m ON m.id = b.message_id \
           INNER JOIN rooms r ON r.id = m.room_id \
          WHERE b.user_id = ? AND m.deleted_at IS NULL AND m.quarantined = 0 \
          ORDER BY b.created_at DESC, b.message_id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BookmarkRow {
            message_id: r.get("message_id"),
            created_at: r.get("created_at"),
            message_body: r.get("message_body"),
            message_created_at: r.get("message_created_at"),
            author_user_id: r.get("author_user_id"),
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            room_type: r.get("room_type"),
        })
        .collect())
}

/// Look up the room id for a message, ignoring soft-deleted messages.
/// `None` for nonexistent or deleted. The bookmark route uses this to
/// resolve the room without trusting client input, then checks viewer
/// membership before allowing the bookmark to be created.
pub async fn room_for_message(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<i64> = sqlx::query_scalar(
        "SELECT room_id FROM messages WHERE id = ? AND deleted_at IS NULL AND quarantined = 0",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
