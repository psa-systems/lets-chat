//! LC-495: workflow-automation rule persistence (chat domain).
//!
//! Rows in `room_automations` are room-scoped "when X, do Y" rules. The read
//! side is driven by the engine (`crate::automations`) on the post / reaction
//! hot paths; the write side by the manage UI (`routes::automations`). See the
//! migration (`0079_room_automations.sql`) for the column semantics.

use sqlx::{Row, SqlitePool};

/// A single automation rule.
#[derive(Debug, Clone)]
pub struct RoomAutomation {
    pub id: i64,
    pub room_id: i64,
    pub name: Option<String>,
    pub enabled: bool,
    pub trigger_kind: String,
    /// Trigger-specific filter; `None` = fire on every occurrence.
    pub match_text: Option<String>,
    pub action_kind: String,
    pub action_body: String,
    pub created_by: String,
    pub created_at: String,
}

fn row_to_automation(row: sqlx::sqlite::SqliteRow) -> RoomAutomation {
    RoomAutomation {
        id: row.get("id"),
        room_id: row.get("room_id"),
        name: row.get("name"),
        enabled: row.get::<i64, _>("enabled") != 0,
        trigger_kind: row.get("trigger_kind"),
        match_text: row.get("match_text"),
        action_kind: row.get("action_kind"),
        action_body: row.get("action_body"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    }
}

const COLS: &str = "id, room_id, name, enabled, trigger_kind, match_text, \
                    action_kind, action_body, created_by, created_at";

/// Every rule for a room (enabled and disabled), newest first, for the manage UI.
pub async fn list_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<Vec<RoomAutomation>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM room_automations WHERE room_id = ? ORDER BY created_at DESC, id DESC"
    ))
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_automation).collect())
}

/// Enabled rules for a room that listen for `trigger_kind` - the engine's hot
/// path. Oldest first so rules fire in creation order.
pub async fn list_active_for_trigger(
    pool: &SqlitePool,
    room_id: i64,
    trigger_kind: &str,
) -> Result<Vec<RoomAutomation>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM room_automations \
         WHERE room_id = ? AND trigger_kind = ? AND enabled = 1 \
         ORDER BY created_at ASC, id ASC"
    ))
    .bind(room_id)
    .bind(trigger_kind)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_automation).collect())
}

/// Fetch one rule (used to scope toggle/delete to the rule's own room).
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<RoomAutomation>, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT {COLS} FROM room_automations WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(row_to_automation))
}

/// Number of rules a room has (callers cap this before inserting).
pub async fn count_for_room(pool: &SqlitePool, room_id: i64) -> Result<i64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_automations WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Insert a new rule, returning its id.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &SqlitePool,
    room_id: i64,
    name: Option<&str>,
    trigger_kind: &str,
    match_text: Option<&str>,
    action_kind: &str,
    action_body: &str,
    created_by: &str,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query(
        "INSERT INTO room_automations \
         (room_id, name, trigger_kind, match_text, action_kind, action_body, created_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(name)
    .bind(trigger_kind)
    .bind(match_text)
    .bind(action_kind)
    .bind(action_body)
    .bind(created_by)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// Toggle a rule on/off, scoped to its room (so a forged id from another room
/// is a no-op). Returns rows affected.
pub async fn set_enabled(
    pool: &SqlitePool,
    id: i64,
    room_id: i64,
    enabled: bool,
) -> Result<u64, sqlx::Error> {
    let n = sqlx::query("UPDATE room_automations SET enabled = ? WHERE id = ? AND room_id = ?")
        .bind(enabled as i64)
        .bind(id)
        .bind(room_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n)
}

/// Delete a rule, scoped to its room. Returns rows affected.
pub async fn delete(pool: &SqlitePool, id: i64, room_id: i64) -> Result<u64, sqlx::Error> {
    let n = sqlx::query("DELETE FROM room_automations WHERE id = ? AND room_id = ?")
        .bind(id)
        .bind(room_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n)
}
