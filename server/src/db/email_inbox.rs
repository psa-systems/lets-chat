//! LC-77: per-room email-ingress inboxes. The email-shaped sibling of LC-74
//! incoming webhooks (`crate::db::webhooks`). An external sender mails the
//! polled mailbox at `<token>@<ingress-domain>`; the IMAP poll loop resolves
//! the token to a row in this table, hash-matches the secret, and posts the
//! body as a synthetic "Email" actor message.
//!
//! Only the HMAC of an inbox secret is stored (keyed by the server secret;
//! same `crate::auth::hash_api_token` shape webhooks use). The plaintext
//! token lives only in the address shown ONCE at inbox creation.

use sqlx::{Row, SqlitePool};

/// An inbox row for the room management page (no secret material).
#[derive(Debug, Clone)]
pub struct EmailInboxRow {
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// Fields the IMAP poll handler needs to validate + attribute a polled
/// message. Mirrors `crate::db::webhooks::WebhookAuth` field-for-field.
#[derive(Debug, Clone)]
pub struct EmailInboxAuth {
    pub id: i64,
    pub room_id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub revoked_at: Option<String>,
}

/// Display identity for rendering an email-ingress-authored message.
/// Mirrors `crate::db::webhooks::WebhookIdentity` field-for-field so the
/// render layer can drop in alongside the webhook arm.
#[derive(Debug, Clone)]
pub struct EmailInboxIdentity {
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
        "INSERT INTO email_inboxes (room_id, name, avatar_url, secret_hash, created_by) \
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

/// Look up an inbox by its hashed secret (for the inbound IMAP poll path).
pub async fn find_by_secret_hash(
    pool: &SqlitePool,
    secret_hash: &str,
) -> sqlx::Result<Option<EmailInboxAuth>> {
    let row = sqlx::query(
        "SELECT id, room_id, name, avatar_url, revoked_at \
         FROM email_inboxes WHERE secret_hash = ?",
    )
    .bind(secret_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| EmailInboxAuth {
        id: r.get("id"),
        room_id: r.get("room_id"),
        name: r.get("name"),
        avatar_url: r.get("avatar_url"),
        revoked_at: r.get("revoked_at"),
    }))
}

/// Display identity for an inbox id (rendering an email-ingress message).
/// `None` if the inbox row was hard-deleted; with the `ON DELETE SET NULL`
/// FK on `messages.email_inbox_id`, the caller reaches this only when the
/// column is set, but the inbox row may still be missing in race-with-delete
/// scenarios.
pub async fn identity(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<EmailInboxIdentity>> {
    let row = sqlx::query("SELECT name, avatar_url FROM email_inboxes WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| EmailInboxIdentity {
        name: r.get("name"),
        avatar_url: r.get("avatar_url"),
    }))
}

/// Inboxes for a room's management page, newest first.
pub async fn list_for_room(pool: &SqlitePool, room_id: i64) -> sqlx::Result<Vec<EmailInboxRow>> {
    let rows = sqlx::query(
        "SELECT id, name, avatar_url, created_at, last_used_at, revoked_at \
         FROM email_inboxes WHERE room_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EmailInboxRow {
            id: r.get("id"),
            name: r.get("name"),
            avatar_url: r.get("avatar_url"),
            created_at: r.get("created_at"),
            last_used_at: r.get("last_used_at"),
            revoked: r.get::<Option<String>, _>("revoked_at").is_some(),
        })
        .collect())
}

/// Revoke an inbox in a room. Sets `revoked_at` (row retained for audit).
/// Scoped by room_id so a moderator can only revoke their room's inboxes.
pub async fn revoke(pool: &SqlitePool, id: i64, room_id: i64) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE email_inboxes SET revoked_at = datetime('now') \
         WHERE id = ? AND room_id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Best-effort `last_used_at` bump; called from the poll loop after a
/// successful post.
pub async fn touch_last_used(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE email_inboxes SET last_used_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
