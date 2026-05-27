//! LC-78: data access for the `bridges` table. Chunk 2 only needs
//! `find_by_id` (the bridge-messages endpoint loads the row to read kind +
//! bot_user_id + room_id + status). Chunk 4 will add insert / list / remove /
//! heartbeat / set_status / seal-unseal-config helpers for the admin UI.

use crate::models::Bridge;
use sqlx::{Row, SqlitePool};

fn map(row: &sqlx::sqlite::SqliteRow) -> Bridge {
    Bridge {
        id: row.get("id"),
        room_id: row.get("room_id"),
        kind: row.get("kind"),
        bot_user_id: row.get("bot_user_id"),
        status: row.get("status"),
        last_heartbeat_at: row.get("last_heartbeat_at"),
        last_error: row.get("last_error"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    }
}

/// Find a bridge by id. Returns `None` if no row exists; the caller
/// translates that to `AppError::NotFound`. Does NOT load the sealed
/// config (chunk 4 will add a separate helper that returns the
/// unsealed config under `LETS_CHAT_SECRET_KEY`, used only by the
/// admin UI). The bridge-messages endpoint never needs the config.
pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Bridge>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, kind, bot_user_id, status, last_heartbeat_at, last_error, created_by, created_at \
         FROM bridges WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map))
}
