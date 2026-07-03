//! LC-527: trackable follow-up tasks. A follow-up list is anchored to a
//! `messages` row (mirroring polls, LC-66); `followup_items` carries the
//! checklist. Toggling / claiming an item mirrors poll voting: mutate a row,
//! then broadcast a re-rendered fragment to the room.

use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

/// The anchor row for a follow-up list.
#[derive(Debug, Clone)]
pub struct FollowUp {
    pub message_id: i64,
    pub transcript_id: Option<i64>,
    pub created_by: String,
}

/// One checklist item.
#[derive(Debug, Clone)]
pub struct FollowUpItem {
    pub id: i64,
    pub message_id: i64,
    pub position: i64,
    pub text: String,
    pub assignee_id: Option<String>,
    pub done: bool,
}

/// Create a follow-up list: insert the anchor message, the `followups` row, and
/// the items in a single transaction (atomic, mirrors `polls::create`). Returns
/// the new message id. `body` becomes the message body (a short header) so
/// search / quote / pin keep working.
pub async fn create(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
    transcript_id: Option<i64>,
    items: &[String],
) -> sqlx::Result<i64> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(&mut *tx)
        .await?;
    let message_id = res.last_insert_rowid();
    sqlx::query("INSERT INTO followups (message_id, transcript_id, created_by) VALUES (?, ?, ?)")
        .bind(message_id)
        .bind(transcript_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for (i, text) in items.iter().enumerate() {
        sqlx::query("INSERT INTO followup_items (message_id, position, text) VALUES (?, ?, ?)")
            .bind(message_id)
            .bind(i as i64)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(message_id)
}

/// Which of `message_ids` are follow-up lists (bulk page render, mirrors
/// `polls::poll_message_ids`).
pub async fn followup_message_ids(
    pool: &SqlitePool,
    message_ids: &[i64],
) -> sqlx::Result<HashSet<i64>> {
    if message_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT message_id FROM followups WHERE message_id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("message_id"))
        .collect())
}

/// The anchor row for `message_id`, or `None` when it is not a follow-up list.
pub async fn get(pool: &SqlitePool, message_id: i64) -> sqlx::Result<Option<FollowUp>> {
    let row = sqlx::query(
        "SELECT message_id, transcript_id, created_by FROM followups WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| FollowUp {
        message_id: r.get("message_id"),
        transcript_id: r.get("transcript_id"),
        created_by: r.get("created_by"),
    }))
}

/// Ordered checklist items for a list.
pub async fn items(pool: &SqlitePool, message_id: i64) -> sqlx::Result<Vec<FollowUpItem>> {
    let rows = sqlx::query(
        "SELECT id, message_id, position, text, assignee_id, done \
         FROM followup_items WHERE message_id = ? ORDER BY position, id",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| FollowUpItem {
            id: r.get("id"),
            message_id: r.get("message_id"),
            position: r.get("position"),
            text: r.get("text"),
            assignee_id: r.get("assignee_id"),
            done: r.get::<i64, _>("done") != 0,
        })
        .collect())
}

/// A single item (for the toggle / claim handlers to resolve its room).
pub async fn item(pool: &SqlitePool, item_id: i64) -> sqlx::Result<Option<FollowUpItem>> {
    let row = sqlx::query(
        "SELECT id, message_id, position, text, assignee_id, done \
         FROM followup_items WHERE id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| FollowUpItem {
        id: r.get("id"),
        message_id: r.get("message_id"),
        position: r.get("position"),
        text: r.get("text"),
        assignee_id: r.get("assignee_id"),
        done: r.get::<i64, _>("done") != 0,
    }))
}

/// Toggle an item's done state. Marking done stamps `done_by`/`done_at`;
/// clearing wipes them. `acting_user` is recorded as the completer.
pub async fn toggle_done(pool: &SqlitePool, item_id: i64, acting_user: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE followup_items \
         SET done = CASE WHEN done = 0 THEN 1 ELSE 0 END, \
             done_by = CASE WHEN done = 0 THEN ? ELSE NULL END, \
             done_at = CASE WHEN done = 0 THEN datetime('now') ELSE NULL END \
         WHERE id = ?",
    )
    .bind(acting_user)
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Self-claim toggle: if `user_id` already holds the item, release it;
/// otherwise assign it to `user_id`. Self-claim only, so a user can never
/// assign work to someone else.
pub async fn toggle_claim(pool: &SqlitePool, item_id: i64, user_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE followup_items \
         SET assignee_id = CASE WHEN assignee_id = ? THEN NULL ELSE ? END \
         WHERE id = ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}
