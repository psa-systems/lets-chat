//! LC-74: incoming webhooks. Only the HMAC of a webhook secret is stored;
//! see `crate::auth::hash_api_token` for the keying. The plaintext secret
//! lives only in the URL shown once at creation.

use sqlx::{Row, SqlitePool};

/// A webhook row for the room management page (no secret material).
#[derive(Debug, Clone)]
pub struct WebhookRow {
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// Fields the incoming handler needs to validate + attribute a POST.
#[derive(Debug, Clone)]
pub struct WebhookAuth {
    pub id: i64,
    pub room_id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub revoked_at: Option<String>,
}

/// Display identity for rendering a webhook-authored message.
#[derive(Debug, Clone)]
pub struct WebhookIdentity {
    pub name: String,
    pub avatar_url: Option<String>,
}

pub async fn insert(
    pool: &SqlitePool,
    room_id: i64,
    name: &str,
    avatar_url: Option<&str>,
    secret_hash: &str,
    created_by: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO incoming_webhooks (room_id, name, avatar_url, secret_hash, created_by) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(name)
    .bind(avatar_url)
    .bind(secret_hash)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Look up a webhook by its hashed secret (for the incoming POST).
pub async fn find_by_secret_hash(
    pool: &SqlitePool,
    secret_hash: &str,
) -> sqlx::Result<Option<WebhookAuth>> {
    let row = sqlx::query(
        "SELECT id, room_id, name, avatar_url, revoked_at \
         FROM incoming_webhooks WHERE secret_hash = ?",
    )
    .bind(secret_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| WebhookAuth {
        id: r.get("id"),
        room_id: r.get("room_id"),
        name: r.get("name"),
        avatar_url: r.get("avatar_url"),
        revoked_at: r.get("revoked_at"),
    }))
}

/// Display identity for a webhook id (rendering a webhook message).
pub async fn identity(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<WebhookIdentity>> {
    let row = sqlx::query("SELECT name, avatar_url FROM incoming_webhooks WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| WebhookIdentity {
        name: r.get("name"),
        avatar_url: r.get("avatar_url"),
    }))
}

/// Webhooks for a room's management page, newest first.
pub async fn list_for_room(pool: &SqlitePool, room_id: i64) -> sqlx::Result<Vec<WebhookRow>> {
    let rows = sqlx::query(
        "SELECT id, name, avatar_url, created_at, last_used_at, revoked_at \
         FROM incoming_webhooks WHERE room_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| WebhookRow {
            id: r.get("id"),
            name: r.get("name"),
            avatar_url: r.get("avatar_url"),
            created_at: r.get("created_at"),
            last_used_at: r.get("last_used_at"),
            revoked: r.get::<Option<String>, _>("revoked_at").is_some(),
        })
        .collect())
}

/// Revoke a webhook in a room. Sets `revoked_at` (row retained for audit).
/// Scoped by room_id so a moderator can only revoke their room's webhooks.
pub async fn revoke(pool: &SqlitePool, id: i64, room_id: i64) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE incoming_webhooks SET revoked_at = datetime('now') \
         WHERE id = ? AND room_id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Best-effort `last_used_at` bump; called from a detached task.
pub async fn touch_last_used(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE incoming_webhooks SET last_used_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
