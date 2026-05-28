//! LC-78: data access for the `bridges` table. The sealed daemon config
//! follows the LC-77 `imap_inbox_config` / `vapid_keys` two-column convention
//! (`_encrypted` + `_nonce`) under `LETS_CHAT_SECRET_KEY`; a chat.db leak
//! cannot reconstruct usable Matrix shared secrets.
//!
//! Removal under STOP-NEW lifecycle: deleting a bridges row triggers
//! `ON DELETE SET NULL` on `messages.bridge_id`, but the row-snapshotted
//! `bridge_foreign_name` + `bridge_kind` columns persist so historical
//! render still shows "alice (via matrix)". The render-side resolver gates
//! on `bridge_foreign_name`, not `bridge_id`, so post-removal messages
//! render correctly.

use crate::crypto;
use crate::models::Bridge;
use sqlx::{Row, SqlitePool};

#[derive(Debug, thiserror::Error)]
pub enum BridgeConfigError {
    #[error("crypto: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("sql: {0}")]
    Sql(#[from] sqlx::Error),
}

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
/// config; use `read_config` for that (admin-only readback path).
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

/// All bridges, newest first. Used by the admin UI's global listing.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Bridge>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, kind, bot_user_id, status, last_heartbeat_at, last_error, created_by, created_at \
         FROM bridges ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map).collect())
}

/// Bridges registered to a specific room.
pub async fn list_for_room(pool: &SqlitePool, room_id: i64) -> Result<Vec<Bridge>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, kind, bot_user_id, status, last_heartbeat_at, last_error, created_by, created_at \
         FROM bridges WHERE room_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map).collect())
}

/// Register a new bridge. The `config_plaintext` blob is opaque to the
/// server (typically a daemon-specific JSON document) and is sealed
/// under the process secret key before storage.
pub async fn insert(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    room_id: i64,
    kind: &str,
    config_plaintext: &[u8],
    bot_user_id: &str,
    created_by: &str,
) -> Result<i64, BridgeConfigError> {
    let (encrypted, nonce) = crypto::seal(secret_key, config_plaintext)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO bridges (room_id, kind, config_encrypted, config_nonce, bot_user_id, created_by) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(room_id)
    .bind(kind)
    .bind(&encrypted)
    .bind(&nonce)
    .bind(bot_user_id)
    .bind(created_by)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Read and decrypt the daemon config blob. Admin-only path; the
/// bridge-messages endpoint never needs the config. Returns `None` if
/// the bridge row does not exist.
pub async fn read_config(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    id: i64,
) -> Result<Option<Vec<u8>>, BridgeConfigError> {
    let row = sqlx::query("SELECT config_encrypted, config_nonce FROM bridges WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(r) = row else { return Ok(None) };
    let encrypted: Vec<u8> = r.get("config_encrypted");
    let nonce: Vec<u8> = r.get("config_nonce");
    let plaintext = crypto::open(secret_key, &nonce, &encrypted)?;
    Ok(Some(plaintext))
}

/// Remove a bridge. ON DELETE SET NULL on `messages.bridge_id` preserves
/// historical render via the snapshotted `bridge_foreign_name` + `bridge_kind`
/// columns; the resolver keys on `bridge_foreign_name`, not `bridge_id`, so
/// "alice (via matrix)" continues to render correctly after removal. Returns
/// true if a row was deleted, false if no such bridge existed.
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM bridges WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update `last_heartbeat_at` to now and set status. If `error` is `Some`,
/// records it and sets status to `errored`; otherwise clears `last_error`
/// and sets status to `healthy`. Called by the heartbeat endpoint on every
/// daemon ping. Returns true if a row was updated.
pub async fn record_heartbeat(
    pool: &SqlitePool,
    id: i64,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let (status, error_val) = match error {
        Some(e) => ("errored", Some(e)),
        None => ("healthy", None),
    };
    let result = sqlx::query(
        "UPDATE bridges SET last_heartbeat_at = datetime('now'), status = ?, last_error = ? \
         WHERE id = ?",
    )
    .bind(status)
    .bind(error_val)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Forcibly transition a bridge's status (used by the admin stale-sweep in
/// chunk 7 to mark bridges as `stale` when no heartbeat has arrived within
/// the threshold). Does NOT touch `last_heartbeat_at`.
pub async fn set_status(
    pool: &SqlitePool,
    id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE bridges SET status = ?, last_error = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
