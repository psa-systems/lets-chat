//! LC-549: storage for per-message text embeddings (semantic / related search).
//!
//! One row per message in the `message_embeddings` sidecar table. Vectors are
//! little-endian `f32` BLOBs (`embeddings::vec_to_bytes`). Ranking loads a room's
//! rows and cosine-scans them in Rust (`embeddings::cosine_similarity`), which is
//! fine at self-host scale and keeps the query index-simple.

use sqlx::{Row, SqlitePool};

/// One stored embedding: the message it belongs to and its decoded vector.
pub struct StoredEmbedding {
    pub message_id: i64,
    pub vec: Vec<f32>,
}

/// Insert or replace the embedding for `message_id`. `vec` is the little-endian
/// byte encoding from [`crate::embeddings::vec_to_bytes`]; `dim` is its length.
pub async fn upsert(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    dim: i64,
    vec: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_embeddings (message_id, room_id, dim, vec) VALUES (?, ?, ?, ?) \
         ON CONFLICT(message_id) DO UPDATE SET room_id = excluded.room_id, dim = excluded.dim, vec = excluded.vec, created_at = datetime('now')",
    )
    .bind(message_id)
    .bind(room_id)
    .bind(dim)
    .bind(vec)
    .execute(pool)
    .await?;
    Ok(())
}

/// True iff an embedding row already exists for `message_id` (used to skip
/// re-embedding on an edit that did not change the body).
pub async fn exists(pool: &SqlitePool, message_id: i64) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM message_embeddings WHERE message_id = ?")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Fetch the decoded embedding for one message, if present.
pub async fn get(pool: &SqlitePool, message_id: i64) -> Result<Option<Vec<f32>>, sqlx::Error> {
    let row = sqlx::query("SELECT vec FROM message_embeddings WHERE message_id = ?")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| {
        let bytes: Vec<u8> = r.get("vec");
        crate::embeddings::bytes_to_vec(&bytes)
    }))
}

/// LC-673: up to `limit` visible, non-system messages with a non-empty body and
/// no embedding yet, newest-first. Drives the embeddings backfill for history
/// posted before an embeddings endpoint was configured. Matches the timeline's
/// visibility filter (`deleted_at IS NULL AND quarantined = 0`) so the backfill
/// never embeds a message the search would not surface anyway.
pub async fn list_unembedded(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<(i64, i64, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT m.id, m.room_id, m.body FROM messages m \
         LEFT JOIN message_embeddings e ON e.message_id = m.id \
         WHERE e.message_id IS NULL AND m.is_system = 0 \
           AND m.deleted_at IS NULL AND m.quarantined = 0 AND TRIM(m.body) <> '' \
         ORDER BY m.id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("id"), r.get("room_id"), r.get("body")))
        .collect())
}

/// Load every stored embedding for a room, optionally excluding one message
/// (the query message itself). Decoded into vectors ready for ranking.
pub async fn list_for_room(
    pool: &SqlitePool,
    room_id: i64,
    exclude_message_id: Option<i64>,
) -> Result<Vec<StoredEmbedding>, sqlx::Error> {
    let rows = sqlx::query("SELECT message_id, vec FROM message_embeddings WHERE room_id = ?")
        .bind(room_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let message_id: i64 = r.get("message_id");
            if Some(message_id) == exclude_message_id {
                return None;
            }
            let bytes: Vec<u8> = r.get("vec");
            Some(StoredEmbedding {
                message_id,
                vec: crate::embeddings::bytes_to_vec(&bytes),
            })
        })
        .collect())
}
