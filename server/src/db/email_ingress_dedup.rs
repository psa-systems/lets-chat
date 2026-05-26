//! LC-77-MID-DEDUP (#202): exactly-once dedup for the IMAP poll loop.
//!
//! v1's poll posture is at-least-once with `\Seen`-after-attempt: a
//! crash between (process) and (STORE +Seen) on the same UID
//! reprocesses the message on the next tick and posts a duplicate.
//! This module's table records the HMAC of every successfully-processed
//! Message-ID so a re-fetch of the same UID drops with
//! `DropReason::Duplicate` instead of posting again.
//!
//! Privacy: the table stores ONLY the HMAC of the Message-ID under
//! `LETS_CHAT_SECRET_KEY`, never the plaintext. An operator with DB read
//! access cannot enumerate which correspondents the deployment has been
//! receiving mail from.
//!
//! Sweep: rows older than 30 days are dropped opportunistically by the
//! hourly orphan sweeper (see `spawn_orphan_sweeper` in `main.rs`). A
//! Message-ID re-issued more than 30 days after the original is treated
//! as a fresh message; this matches typical mail-system Message-ID
//! lifetimes (most senders use a timestamp + nonce that never recurs).

use sqlx::SqlitePool;

/// HMAC-SHA256 the Message-ID header value under the server secret key.
/// Same primitive `auth::hash_api_token` uses for per-room inbox
/// secrets and webhook secrets, so the dedup table's hash format is
/// consistent with the rest of the at-rest hash surface.
pub fn hash_message_id(secret_key: &[u8; 32], message_id: &str) -> String {
    crate::auth::hash_api_token(secret_key, message_id)
}

/// Check whether a Message-ID hash has already been recorded. Returns
/// `true` when the row exists (the message has been processed before).
pub async fn is_processed(pool: &SqlitePool, message_id_hash: &str) -> sqlx::Result<bool> {
    let row: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM processed_message_ids WHERE message_id_hash = ?")
            .bind(message_id_hash)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Record a successfully-processed Message-ID hash. Returns `true` when
/// a new row was inserted, `false` when the hash was already present
/// (the caller raced another processor for the same UID; rare but
/// possible with two concurrent polls against the same mailbox).
/// Implemented as `INSERT OR IGNORE` so two concurrent inserts both
/// succeed without one failing on the UNIQUE constraint.
pub async fn mark_processed(pool: &SqlitePool, message_id_hash: &str) -> sqlx::Result<bool> {
    let res =
        sqlx::query("INSERT OR IGNORE INTO processed_message_ids (message_id_hash) VALUES (?)")
            .bind(message_id_hash)
            .execute(pool)
            .await?;
    Ok(res.rows_affected() > 0)
}

/// Drop every row whose `processed_at` is older than `days` days.
/// Returns the count of rows deleted for the orphan sweeper's log line.
pub async fn sweep_old(pool: &SqlitePool, days: i64) -> sqlx::Result<u64> {
    let cutoff = format!("-{days} days");
    let res =
        sqlx::query("DELETE FROM processed_message_ids WHERE processed_at < datetime('now', ?)")
            .bind(&cutoff)
            .execute(pool)
            .await?;
    Ok(res.rows_affected())
}
