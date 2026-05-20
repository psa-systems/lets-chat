//! LC-63: message reminders. Mirrors the scheduled-messages shape: a row
//! is pending while `fired_at IS NULL`; the dispatcher claims it by
//! flipping `fired_at` to now, and a dispatch failure leaves it NULL so
//! the next tick retries. The `message_id` FK cascades, so deleting a
//! message removes its reminders (they never fire).

use sqlx::{Row, SqlitePool};

/// Cap on rows a single dispatcher tick will claim, so a backlog cannot
/// stall the loop. The poll runs every 30s; 100/tick clears any realistic
/// queue quickly without a long single transaction.
const DUE_BATCH: i64 = 100;

/// A due reminder joined with its message + room, ready to notify.
#[derive(Debug, Clone)]
pub struct DueReminder {
    pub id: i64,
    pub user_id: String,
    pub message_id: i64,
    pub room_id: i64,
    pub room_type: String,
    pub room_name: String,
    pub snippet: String,
}

/// One row for the `/reminders` management list.
#[derive(Debug, Clone)]
pub struct ReminderRow {
    pub id: i64,
    pub message_id: i64,
    pub room_id: i64,
    pub room_type: String,
    pub room_name: String,
    pub snippet: String,
    pub remind_at: String,
    pub fired_at: Option<String>,
}

/// Insert a pending reminder. `remind_at` is the SQLite-format UTC string.
pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    message_id: i64,
    remind_at: &str,
) -> sqlx::Result<i64> {
    let res =
        sqlx::query("INSERT INTO reminders (user_id, message_id, remind_at) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(message_id)
            .bind(remind_at)
            .execute(pool)
            .await?;
    Ok(res.last_insert_rowid())
}

/// Confirm a message exists (so a reminder is never inserted against a
/// missing/deleted message). Returns the room id when present.
pub async fn message_room(pool: &SqlitePool, message_id: i64) -> sqlx::Result<Option<i64>> {
    let row = sqlx::query("SELECT room_id FROM messages WHERE id = ? AND deleted_at IS NULL")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("room_id")))
}

fn snippet_of(body: &str) -> String {
    let trimmed = body.trim();
    let s: String = trimmed.chars().take(140).collect();
    if trimmed.chars().count() > 140 {
        format!("{s}…")
    } else {
        s
    }
}

/// Due, not-yet-fired reminders joined with message + room context. Skips
/// reminders whose message was soft-deleted (the FK only cascades on hard
/// delete; messages are soft-deleted, so guard here too).
pub async fn select_due(pool: &SqlitePool) -> sqlx::Result<Vec<DueReminder>> {
    let rows = sqlx::query(
        "SELECT rem.id, rem.user_id, rem.message_id, m.room_id, r.room_type, r.name AS room_name, m.body \
         FROM reminders rem \
         JOIN messages m ON m.id = rem.message_id \
         JOIN rooms r ON r.id = m.room_id \
         WHERE rem.fired_at IS NULL AND rem.remind_at <= datetime('now') AND m.deleted_at IS NULL \
         ORDER BY rem.remind_at \
         LIMIT ?",
    )
    .bind(DUE_BATCH)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DueReminder {
            id: r.get("id"),
            user_id: r.get("user_id"),
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            room_type: r.get("room_type"),
            room_name: r.get("room_name"),
            snippet: snippet_of(&r.get::<String, _>("body")),
        })
        .collect())
}

/// Atomically claim a reminder: flip `fired_at` from NULL to now. Returns
/// true only for the claimer, so a reminder fires at most once even if a
/// tick overlaps.
pub async fn mark_fired(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE reminders SET fired_at = datetime('now') WHERE id = ? AND fired_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

fn map_row(r: sqlx::sqlite::SqliteRow) -> ReminderRow {
    ReminderRow {
        id: r.get("id"),
        message_id: r.get("message_id"),
        room_id: r.get("room_id"),
        room_type: r.get("room_type"),
        room_name: r.get("room_name"),
        snippet: snippet_of(&r.get::<String, _>("body")),
        remind_at: r.get("remind_at"),
        fired_at: r.get("fired_at"),
    }
}

/// Pending reminders for a user, soonest-first. Soft-deleted messages are
/// excluded so a cancelled-by-deletion reminder does not linger in the UI.
pub async fn list_pending_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> sqlx::Result<Vec<ReminderRow>> {
    let rows = sqlx::query(
        "SELECT rem.id, rem.message_id, m.room_id, r.room_type, r.name AS room_name, m.body, \
                rem.remind_at, rem.fired_at \
         FROM reminders rem \
         JOIN messages m ON m.id = rem.message_id \
         JOIN rooms r ON r.id = m.room_id \
         WHERE rem.user_id = ? AND rem.fired_at IS NULL AND m.deleted_at IS NULL \
         ORDER BY rem.remind_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_row).collect())
}

/// Recently-fired reminders for a user, most-recent-first.
pub async fn list_recent_fired_for_user(
    pool: &SqlitePool,
    user_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<ReminderRow>> {
    let rows = sqlx::query(
        "SELECT rem.id, rem.message_id, m.room_id, r.room_type, r.name AS room_name, m.body, \
                rem.remind_at, rem.fired_at \
         FROM reminders rem \
         JOIN messages m ON m.id = rem.message_id \
         JOIN rooms r ON r.id = m.room_id \
         WHERE rem.user_id = ? AND rem.fired_at IS NOT NULL AND m.deleted_at IS NULL \
         ORDER BY rem.fired_at DESC \
         LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_row).collect())
}

/// Cancel (delete) a reminder the caller owns. Returns true when a row was
/// removed. Owner-scoped so one user cannot cancel another's reminder.
pub async fn delete(pool: &SqlitePool, id: i64, user_id: &str) -> sqlx::Result<bool> {
    let res = sqlx::query("DELETE FROM reminders WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Delete every reminder owned by a user (account purge). Reminders ON the
/// user's own messages already cascade when those messages are deleted;
/// this also clears reminders the user set on other people's messages.
pub async fn delete_all_for_user(pool: &SqlitePool, user_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM reminders WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}
