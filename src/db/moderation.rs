use sqlx::{Row, SqlitePool};

use crate::models::mod_action::ModAction;

/// Record a moderation action in the audit log (chat.db).
pub async fn log_mod_action(
    pool: &SqlitePool,
    action: &str,
    target_user: &str,
    actor_user: &str,
    reason: Option<&str>,
    room_id: Option<i64>,
    metadata: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO mod_actions (action, target_user, actor_user, reason, room_id, metadata) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(action)
    .bind(target_user)
    .bind(actor_user)
    .bind(reason)
    .bind(room_id)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// List mod actions ordered by most recent first.
pub async fn list_mod_actions(pool: &SqlitePool) -> Result<Vec<ModAction>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, action, target_user, actor_user, reason, room_id, metadata, created_at \
         FROM mod_actions ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ModAction {
            id: r.get("id"),
            action: r.get("action"),
            target_user: r.get("target_user"),
            actor_user: r.get("actor_user"),
            reason: r.get("reason"),
            room_id: r.get("room_id"),
            metadata: r.get("metadata"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Soft-delete a message by setting deleted_at and deleted_by.
pub async fn soft_delete_message(
    pool: &SqlitePool,
    message_id: i64,
    deleted_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET deleted_at = datetime('now'), deleted_by = ? WHERE id = ?")
        .bind(deleted_by)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}
