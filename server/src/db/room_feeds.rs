//! LC-102: per-room RSS/Atom + iCal feed tokens. Only the HMAC of a feed token
//! is stored (see `crate::auth::hash_api_token`); the plaintext lives only in
//! the URL shown once at creation. A feed is bound to its creator, whose room
//! access is re-checked on every fetch.

use sqlx::{Row, SqlitePool};

/// Fields the public feed handler needs to authorize + scope a fetch.
#[derive(Debug, Clone)]
pub struct FeedAuth {
    pub id: i64,
    pub room_id: i64,
    pub kind: String,
    pub created_by: String,
    pub revoked_at: Option<String>,
}

/// A feed row for the room management page (no token material).
#[derive(Debug, Clone)]
pub struct FeedRow {
    pub id: i64,
    pub kind: String,
    pub created_at: String,
    pub last_fetched_at: Option<String>,
    pub revoked: bool,
}

pub async fn insert(
    pool: &SqlitePool,
    room_id: i64,
    kind: &str,
    token_hash: &str,
    created_by: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO room_feeds (room_id, kind, token_hash, created_by) VALUES (?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(kind)
    .bind(token_hash)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Look up a feed by its hashed token (for the public fetch).
pub async fn find_by_token_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> sqlx::Result<Option<FeedAuth>> {
    let row = sqlx::query(
        "SELECT id, room_id, kind, created_by, revoked_at FROM room_feeds WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| FeedAuth {
        id: r.get("id"),
        room_id: r.get("room_id"),
        kind: r.get("kind"),
        created_by: r.get("created_by"),
        revoked_at: r.get("revoked_at"),
    }))
}

/// Feeds for a room's management page, newest first.
pub async fn list_for_room(pool: &SqlitePool, room_id: i64) -> sqlx::Result<Vec<FeedRow>> {
    let rows = sqlx::query(
        "SELECT id, kind, created_at, last_fetched_at, revoked_at \
         FROM room_feeds WHERE room_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| FeedRow {
            id: r.get("id"),
            kind: r.get("kind"),
            created_at: r.get("created_at"),
            last_fetched_at: r.get("last_fetched_at"),
            revoked: r.get::<Option<String>, _>("revoked_at").is_some(),
        })
        .collect())
}

/// Revoke a feed in a room. Sets `revoked_at` (row retained for audit), scoped
/// by room_id so a moderator can only revoke their own room's feeds.
pub async fn revoke(pool: &SqlitePool, id: i64, room_id: i64) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE room_feeds SET revoked_at = datetime('now') \
         WHERE id = ? AND room_id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Best-effort `last_fetched_at` bump; called from a detached task.
pub async fn touch_last_fetched(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE room_feeds SET last_fetched_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
