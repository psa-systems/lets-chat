use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use crate::models::Attachment;

/// Wire + storage shape of a voice message's waveform: the recording duration
/// in seconds (`d`) plus the normalized peak amplitudes (`p`). The upload
/// route sanitizes a client-supplied blob into this shape; the read path
/// parses it back out. `d` is `None` when the client could not determine the
/// duration (the player then falls back to its own metadata probe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformBlob {
    pub d: Option<f32>,
    pub p: Vec<f32>,
}

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
    /// Raw waveform JSON (a [`WaveformBlob`]) for voice messages, `None`
    /// otherwise. Parsed into `Attachment::waveform` / `voice_duration` by
    /// `parse_waveform`.
    pub waveform: Option<String>,
    /// LC-483: server-side voice-message transcript, `None` until produced.
    pub transcript: Option<String>,
    /// LC-537: author-provided image alt text, `None` when unset.
    pub alt_text: Option<String>,
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
        waveform: row.get("waveform"),
        transcript: row.get("transcript"),
        alt_text: row.get("alt_text"),
    }
}

/// Parse the stored waveform JSON into `(peaks, duration_seconds)`. A
/// malformed or absent value yields `(None, None)` so a bad row degrades to
/// "audio attachment without a waveform" rather than failing the whole
/// message render.
fn parse_waveform(raw: Option<String>) -> (Option<Vec<f32>>, Option<f32>) {
    match raw.and_then(|r| serde_json::from_str::<WaveformBlob>(&r).ok()) {
        Some(blob) => (Some(blob.p), blob.d),
        None => (None, None),
    }
}

pub async fn insert_upload(
    pool: &SqlitePool,
    uploader_id: &str,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    storage_path: &str,
    waveform: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO file_uploads \
         (uploader_id, filename, mime_type, size_bytes, storage_path, waveform) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uploader_id)
    .bind(filename)
    .bind(mime_type)
    .bind(size_bytes)
    .bind(storage_path)
    .bind(waveform)
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

/// LC-483: store a voice message's server-side transcript on its upload row.
/// Best-effort, overwrites any prior value; the caller transcribes off the
/// request path and only calls this on a non-empty result.
pub async fn set_transcript(
    pool: &SqlitePool,
    upload_id: i64,
    transcript: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE file_uploads SET transcript = ? WHERE id = ?")
        .bind(transcript)
        .bind(upload_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-537: set (or clear, with `None`) the author-provided alt text on an
/// image upload. Authorization is the caller's responsibility; this only
/// touches the row.
pub async fn set_upload_alt_text(
    pool: &SqlitePool,
    upload_id: i64,
    alt_text: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE file_uploads SET alt_text = ? WHERE id = ?")
        .bind(alt_text)
        .bind(upload_id)
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
                u.size_bytes, u.storage_path, u.created_at, u.waveform, u.transcript, \
                u.alt_text, \
                m.room_id AS room_id \
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
        "SELECT id, message_id, filename, mime_type, size_bytes, waveform, transcript, alt_text \
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
        let (waveform, voice_duration) = parse_waveform(r.get("waveform"));
        let att = Attachment {
            id,
            filename: r.get("filename"),
            mime_type: r.get("mime_type"),
            size_bytes: r.get("size_bytes"),
            url: format!("/api/files/{id}"),
            waveform,
            voice_duration,
            transcript: r.get("transcript"),
            alt_text: r.get("alt_text"),
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
///
/// Two exemptions keep intentional orphans alive:
/// - LC-96: rows referenced by `branding.logo_upload_id` - logos are
///   never attached to a message but must survive the sweep.
/// - LC-62: rows referenced by a pending scheduled message - an
///   attachment on a scheduled-for-tomorrow message would otherwise
///   be swept before delivery. `idx_sched_file` keeps that lookup
///   O(1) per candidate.
pub async fn select_orphans_older_than(
    pool: &SqlitePool,
    hours: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    let modifier = format!("-{hours} hours");
    let rows = sqlx::query(
        "SELECT id, storage_path FROM file_uploads \
         WHERE message_id IS NULL \
           AND created_at < datetime('now', ?) \
           AND id NOT IN ( \
               SELECT logo_upload_id FROM branding \
                WHERE logo_upload_id IS NOT NULL \
           ) \
           AND NOT EXISTS ( \
                 SELECT 1 FROM scheduled_messages s \
                  WHERE s.file_id = file_uploads.id \
                    AND s.delivered_at IS NULL \
               )",
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

/// Sum of `size_bytes` across all upload rows. Drives the admin "Uploads"
/// panel total.
pub async fn sum_size_bytes(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM file_uploads")
        .fetch_one(pool)
        .await
}

/// Count of orphan rows (`message_id IS NULL`). Same predicate as the sweep,
/// but cheap enough to compute on every admin-settings render.
pub async fn count_orphans(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM file_uploads WHERE message_id IS NULL")
        .fetch_one(pool)
        .await
}

/// List every image upload row. Used by the admin regenerate-thumbnails
/// action to walk all images and backfill missing previews.
/// LC-87: file-kind filter for the per-room Files tab. `All` returns
/// every upload regardless of mime; the other variants translate to a
/// `mime_type LIKE 'image/%'` / `'video/%'` / `'audio/%'` predicate, or
/// to "pdf" via an exact `application/pdf` match. `Other` returns
/// everything that is not in any of the named buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKindFilter {
    All,
    Image,
    Video,
    Audio,
    Pdf,
    Other,
}

impl FileKindFilter {
    pub fn from_query(s: &str) -> Self {
        match s {
            "image" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            "pdf" => Self::Pdf,
            "other" => Self::Other,
            _ => Self::All,
        }
    }
    pub fn as_query(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Pdf => "pdf",
            Self::Other => "other",
        }
    }
}

/// LC-87: list uploads attached to a room, newest first.
///
/// Filters:
/// - `kind` narrows by mime-type bucket (see [`FileKindFilter`]).
/// - `before_id` is the cursor: when `Some(n)`, only rows with `id < n`
///   are returned. Pass `None` for the first page. The next cursor for
///   the load-more button is the last id of the current page.
/// - `limit` caps the result set; the caller decides the page size.
///
/// Soft-deleted parent messages and orphan uploads (message_id IS NULL)
/// are excluded: the files-tab UX is "files that landed in messages in
/// this room and the message still exists".
pub async fn list_room_files(
    pool: &SqlitePool,
    room_id: i64,
    kind: FileKindFilter,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<UploadRow>, sqlx::Error> {
    // The mime predicate is composed in Rust so we keep one SQL plan per
    // filter rather than baking a CASE into the WHERE clause.
    let mut sql = String::from(
        "SELECT u.id, u.uploader_id, u.message_id, u.filename, u.mime_type, \
                u.size_bytes, u.storage_path, u.created_at, u.waveform, u.transcript, \
                u.alt_text \
           FROM file_uploads u \
           JOIN messages m ON m.id = u.message_id \
          WHERE m.room_id = ? AND m.deleted_at IS NULL AND m.quarantined = 0",
    );
    match kind {
        FileKindFilter::All => {}
        FileKindFilter::Image => sql.push_str(" AND u.mime_type LIKE 'image/%'"),
        FileKindFilter::Video => sql.push_str(" AND u.mime_type LIKE 'video/%'"),
        FileKindFilter::Audio => sql.push_str(" AND u.mime_type LIKE 'audio/%'"),
        FileKindFilter::Pdf => sql.push_str(" AND u.mime_type = 'application/pdf'"),
        FileKindFilter::Other => sql.push_str(
            " AND u.mime_type NOT LIKE 'image/%' \
              AND u.mime_type NOT LIKE 'video/%' \
              AND u.mime_type NOT LIKE 'audio/%' \
              AND u.mime_type != 'application/pdf'",
        ),
    }
    if before_id.is_some() {
        sql.push_str(" AND u.id < ?");
    }
    sql.push_str(" ORDER BY u.id DESC LIMIT ?");

    let mut q = sqlx::query(&sql).bind(room_id);
    if let Some(b) = before_id {
        q = q.bind(b);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows.iter().map(map_upload).collect())
}

pub async fn list_image_uploads(pool: &SqlitePool) -> Result<Vec<UploadRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, uploader_id, message_id, filename, mime_type, size_bytes, \
                storage_path, created_at, waveform, transcript, alt_text \
         FROM file_uploads \
         WHERE mime_type LIKE 'image/%' \
         ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_upload).collect())
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
