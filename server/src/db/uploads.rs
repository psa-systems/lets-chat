use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use crate::models::Attachment;

/// Raw row from `file_uploads`. The route layer maps this into `Attachment`
/// (which is template-safe) and resolves access by joining to messages.
#[derive(Debug, Clone)]
pub struct UploadRow {
    pub id: i64,
    pub uploader_id: String,
    pub message_id: Option<i64>,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_path: String,
    pub created_at: String,
}

fn map_upload(row: &sqlx::sqlite::SqliteRow) -> UploadRow {
    UploadRow {
        id: row.get("id"),
        uploader_id: row.get("uploader_id"),
        message_id: row.get("message_id"),
        filename: row.get("filename"),
        mime_type: row.get("mime_type"),
        size_bytes: row.get("size_bytes"),
        storage_path: row.get("storage_path"),
        created_at: row.get("created_at"),
    }
}

pub async fn insert_upload(
    pool: &SqlitePool,
    uploader_id: &str,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    storage_path: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO file_uploads (uploader_id, filename, mime_type, size_bytes, storage_path) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(uploader_id)
    .bind(filename)
    .bind(mime_type)
    .bind(size_bytes)
    .bind(storage_path)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Promote an orphan upload (`message_id IS NULL`) to belong to a freshly
/// inserted message. Idempotent: re-linking the same row is a no-op.
pub async fn link_upload_to_message(
    pool: &SqlitePool,
    upload_id: i64,
    message_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE file_uploads SET message_id = ? \
         WHERE id = ? AND (message_id IS NULL OR message_id = ?)",
    )
    .bind(message_id)
    .bind(upload_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch one upload row alongside the room_id of its parent message.
/// Returns `None` for unknown ids. The second tuple element is `Some(rid)`
/// once the upload is linked to a message and `None` while it is still
/// orphaned.
pub async fn get_upload(
    pool: &SqlitePool,
    upload_id: i64,
) -> Result<Option<(UploadRow, Option<i64>)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT u.id, u.uploader_id, u.message_id, u.filename, u.mime_type, \
                u.size_bytes, u.storage_path, u.created_at, m.room_id AS room_id \
         FROM file_uploads u \
         LEFT JOIN messages m ON m.id = u.message_id \
         WHERE u.id = ?",
    )
    .bind(upload_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        let upload = map_upload(&r);
        let room_id: Option<i64> = r.get("room_id");
        (upload, room_id)
    }))
}

/// Bulk-load attachments for a slice of message ids. Returns a map keyed by
/// `message_id` so callers can attach to each `MessageView` in O(1).
pub async fn attachments_for_messages(
    pool: &SqlitePool,
    message_ids: &[i64],
) -> Result<HashMap<i64, Vec<Attachment>>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, message_id, filename, mime_type, size_bytes \
         FROM file_uploads \
         WHERE message_id IN ({placeholders}) \
         ORDER BY id"
    );
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let mut out: HashMap<i64, Vec<Attachment>> = HashMap::new();
    for r in rows {
        let id: i64 = r.get("id");
        let message_id: Option<i64> = r.get("message_id");
        let Some(mid) = message_id else { continue };
        let att = Attachment {
            id,
            filename: r.get("filename"),
            mime_type: r.get("mime_type"),
            size_bytes: r.get("size_bytes"),
            url: format!("/api/files/{id}"),
        };
        out.entry(mid).or_default().push(att);
    }
    Ok(out)
}

/// Fetch attachments for a single message id. Convenience wrapper around the
/// bulk loader for fragment-render hot paths (single-message WS broadcasts).
pub async fn attachments_for_message(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<Vec<Attachment>, sqlx::Error> {
    let map = attachments_for_messages(pool, &[message_id]).await?;
    Ok(map.into_iter().next().map(|(_, v)| v).unwrap_or_default())
}

// ── Orphan GC + admin queries ─────────────────────────────────────────────────

/// Select orphans (`message_id IS NULL`) whose `created_at` is older than
/// `hours` ago. Uses `idx_file_uploads_orphan`.
pub async fn select_orphans_older_than(
    pool: &SqlitePool,
    hours: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    let modifier = format!("-{hours} hours");
    let rows = sqlx::query(
        "SELECT id, storage_path FROM file_uploads \
         WHERE message_id IS NULL AND created_at < datetime('now', ?)",
    )
    .bind(modifier)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("id"), r.get("storage_path")))
        .collect())
}

/// Count rows that share `storage_path` with the given row id (excluded).
/// The sweeper calls this inside a `BEGIN IMMEDIATE` transaction to decide
/// whether the on-disk file is still referenced. Takes
/// `&mut SqliteConnection` so the count and the row-delete share the
/// transaction handle.
pub async fn count_uploads_sharing_path(
    conn: &mut sqlx::SqliteConnection,
    storage_path: &str,
    exclude_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM file_uploads WHERE storage_path = ? AND id <> ?")
        .bind(storage_path)
        .bind(exclude_id)
        .fetch_one(&mut *conn)
        .await
}

/// Delete the row at `id`. Used by the orphan sweeper; takes
/// `&mut SqliteConnection` for transaction sharing.
pub async fn delete_upload_row(
    conn: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM file_uploads WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

// ── Link preview cache ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LinkPreviewRow {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub fetched_at: String,
}

pub async fn get_link_preview(
    pool: &SqlitePool,
    url_hash: &str,
) -> Result<Option<LinkPreviewRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT url, title, description, image_url, fetched_at \
         FROM link_previews WHERE url_hash = ?",
    )
    .bind(url_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| LinkPreviewRow {
        url: r.get("url"),
        title: r.get("title"),
        description: r.get("description"),
        image_url: r.get("image_url"),
        fetched_at: r.get("fetched_at"),
    }))
}

pub async fn upsert_link_preview(
    pool: &SqlitePool,
    url_hash: &str,
    url: &str,
    title: Option<&str>,
    description: Option<&str>,
    image_url: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO link_previews (url_hash, url, title, description, image_url, fetched_at) \
         VALUES (?, ?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(url_hash) DO UPDATE SET \
             url = excluded.url, \
             title = excluded.title, \
             description = excluded.description, \
             image_url = excluded.image_url, \
             fetched_at = excluded.fetched_at",
    )
    .bind(url_hash)
    .bind(url)
    .bind(title)
    .bind(description)
    .bind(image_url)
    .execute(pool)
    .await?;
    Ok(())
}
