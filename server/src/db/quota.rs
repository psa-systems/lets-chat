//! Per-user and per-enclave storage quotas (LC-93).
//!
//! Quotas are stored on chat.db so the upload-time check stays
//! single-domain: `user_storage_quotas` holds per-user caps, and the
//! `enclaves.quota_bytes` column (added in migration 0031) holds the
//! per-enclave cap. A NULL / missing row means "unlimited", which is
//! also today's pre-feature behavior.
//!
//! Usage is computed by live SUM over `file_uploads` rather than a
//! cached counter; SQLite reads the small `file_uploads` table fast
//! enough that the per-request cost is acceptable for the volumes a
//! self-hosted deployment sees, and live counts auto-recompute on
//! delete without any trigger or sweep coordination.

use sqlx::{Row, SqlitePool};

/// SUM of `file_uploads.size_bytes` owned by `user_id`. Excludes
/// uploads attached to a system message (`messages.is_system = 1`),
/// matching the LC-93 acceptance criterion that system-generated
/// uploads should not count against the user's cap. Orphan uploads
/// (no message yet) DO count: they still consume disk and they
/// belong to the uploader until the sweep claims them.
pub async fn sum_user_usage(pool: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(u.size_bytes), 0) \
         FROM file_uploads u \
         LEFT JOIN messages m ON m.id = u.message_id \
         WHERE u.uploader_id = ? \
           AND (m.id IS NULL OR m.is_system = 0)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// SUM of `file_uploads.size_bytes` attached to messages in any room
/// of `enclave_id`. Orphan uploads have no room link yet and are not
/// counted here; they only appear against the user.
///
/// Soft-deleted messages ARE counted: the upload row stays in
/// `file_uploads` after a soft delete (the orphan sweeper only claims
/// rows with `message_id IS NULL`), the bytes stay on disk, and
/// excluding them would let a member free enclave headroom by
/// soft-deleting their own old messages. Matches `sum_user_usage`
/// for the same reason - both queries count what is on disk, not
/// what the file browser renders.
pub async fn sum_enclave_usage(pool: &SqlitePool, enclave_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(u.size_bytes), 0) \
         FROM file_uploads u \
         JOIN messages m ON m.id = u.message_id \
         JOIN rooms r ON r.id = m.room_id \
         WHERE r.enclave_id = ? \
           AND m.is_system = 0",
    )
    .bind(enclave_id)
    .fetch_one(pool)
    .await
}

/// Return the user's quota in bytes, or `None` if unlimited.
pub async fn get_user_quota(pool: &SqlitePool, user_id: &str) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT quota_bytes FROM user_storage_quotas WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("quota_bytes")))
}

/// Upsert the user's quota. Pass `None` to remove the cap (back to
/// unlimited), `Some(bytes)` to set it. `bytes` must be `>= 0`; the
/// caller is responsible for validating that.
pub async fn set_user_quota(
    pool: &SqlitePool,
    user_id: &str,
    quota_bytes: Option<i64>,
) -> Result<(), sqlx::Error> {
    match quota_bytes {
        Some(bytes) => {
            sqlx::query(
                "INSERT INTO user_storage_quotas (user_id, quota_bytes, updated_at) \
                 VALUES (?, ?, datetime('now')) \
                 ON CONFLICT(user_id) DO UPDATE SET \
                     quota_bytes = excluded.quota_bytes, \
                     updated_at = excluded.updated_at",
            )
            .bind(user_id)
            .bind(bytes)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM user_storage_quotas WHERE user_id = ?")
                .bind(user_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Return the enclave's quota in bytes, or `None` if unlimited (the
/// `enclaves.quota_bytes` column is nullable; the enclave's own row
/// always exists).
pub async fn get_enclave_quota(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT quota_bytes FROM enclaves WHERE id = ?")
        .bind(enclave_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<i64>, _>("quota_bytes")))
}

/// Set or clear the enclave's quota. The row always exists, so this
/// is a plain UPDATE rather than an upsert.
pub async fn set_enclave_quota(
    pool: &SqlitePool,
    enclave_id: i64,
    quota_bytes: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET quota_bytes = ? WHERE id = ?")
        .bind(quota_bytes)
        .bind(enclave_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Resolve the enclave that owns a room. Returns `None` for DMs and
/// any other room that is not nested under an enclave.
pub async fn enclave_id_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT enclave_id FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<i64>, _>("enclave_id")))
}
