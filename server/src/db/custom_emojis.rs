use sqlx::{Row, SqlitePool};

use crate::models::custom_emoji::{CustomEmoji, EmojiRef};

fn map_row(row: &sqlx::sqlite::SqliteRow) -> CustomEmoji {
    CustomEmoji {
        id: row.get("id"),
        enclave_id: row.get("enclave_id"),
        shortcode: row.get("shortcode"),
        storage_path: row.get("storage_path"),
        mime_type: row.get("mime_type"),
        size_bytes: row.get("size_bytes"),
        uploaded_by: row.get("uploaded_by"),
        created_at: row.get("created_at"),
    }
}

pub async fn insert(
    pool: &SqlitePool,
    enclave_id: i64,
    shortcode: &str,
    storage_path: &str,
    mime_type: &str,
    size_bytes: i64,
    uploaded_by: &str,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO custom_emojis (enclave_id, shortcode, storage_path, mime_type, size_bytes, uploaded_by) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(enclave_id)
    .bind(shortcode)
    .bind(storage_path)
    .bind(mime_type)
    .bind(size_bytes)
    .bind(uploaded_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<CustomEmoji>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, enclave_id, shortcode, storage_path, mime_type, size_bytes, uploaded_by, created_at \
         FROM custom_emojis WHERE id=?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_row))
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM custom_emojis WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<CustomEmoji>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, enclave_id, shortcode, storage_path, mime_type, size_bytes, uploaded_by, created_at \
         FROM custom_emojis WHERE enclave_id=? ORDER BY shortcode COLLATE NOCASE",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_row).collect())
}

/// Render-only projection of an enclave's emoji set. Used by the body and
/// reaction renderers as the lookup map for `:shortcode:` resolution.
pub async fn refs_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<EmojiRef>, sqlx::Error> {
    let rows = sqlx::query("SELECT id, shortcode FROM custom_emojis WHERE enclave_id=?")
        .bind(enclave_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| EmojiRef {
            id: r.get("id"),
            shortcode: r.get("shortcode"),
        })
        .collect())
}

/// Render-only projection of every emoji from enclaves that have opted in
/// to global sharing. Used as a fallback layer for DMs and as an additive
/// layer for enclave rooms (the room's own emojis win on shortcode
/// collisions; see `refs_for_room`).
pub async fn refs_globally_shared(pool: &SqlitePool) -> Result<Vec<EmojiRef>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT ce.id, ce.shortcode \
         FROM custom_emojis ce \
         JOIN enclaves e ON e.id = ce.enclave_id \
         WHERE e.share_emojis_globally = 1 \
         ORDER BY ce.enclave_id, ce.shortcode",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EmojiRef {
            id: r.get("id"),
            shortcode: r.get("shortcode"),
        })
        .collect())
}

/// Resolve the emoji set visible inside a single room. The room's own
/// enclave emojis are layered on top of every globally-shared set, so a
/// shortcode defined in the room's enclave always wins over an identical
/// shortcode shared from elsewhere. DMs and rooms without an enclave get
/// the globally-shared set only.
pub async fn refs_for_room(pool: &SqlitePool, room_id: i64) -> Result<Vec<EmojiRef>, sqlx::Error> {
    let row = sqlx::query("SELECT enclave_id FROM rooms WHERE id=?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    let enclave_id: Option<i64> = row.and_then(|r| r.get::<Option<i64>, _>("enclave_id"));
    let shared = refs_globally_shared(pool).await?;
    let Some(eid) = enclave_id else {
        return Ok(shared);
    };
    let own = refs_for_enclave(pool, eid).await?;
    Ok(merge_with_own_precedence(own, shared))
}

/// Merge two emoji sets, preferring rows from `own` whenever a shortcode
/// collides with `shared`. Order: own first (sorted), then shared rows
/// whose shortcode is not already covered.
fn merge_with_own_precedence(own: Vec<EmojiRef>, shared: Vec<EmojiRef>) -> Vec<EmojiRef> {
    use std::collections::HashSet;
    let own_codes: HashSet<String> = own.iter().map(|e| e.shortcode.clone()).collect();
    let mut out = own;
    for s in shared {
        if !own_codes.contains(&s.shortcode) {
            out.push(s);
        }
    }
    out
}
