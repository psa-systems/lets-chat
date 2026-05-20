//! LC-72: scoped personal API tokens. Only the HMAC of the token is stored;
//! see `crate::auth` for hashing + the `ApiAuth` extractor.

use sqlx::{Row, SqlitePool};

/// A token row as shown on the management page (no secret material).
#[derive(Debug, Clone)]
pub struct ApiTokenRow {
    pub id: i64,
    pub name: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// The fields the auth extractor needs to validate a presented token.
#[derive(Debug, Clone)]
pub struct ApiTokenAuth {
    pub id: i64,
    pub user_id: String,
    pub scopes: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// Insert a new token. `token_hash` is the hex HMAC; `scopes` is the
/// space-separated list; `expires_at` is a SQLite datetime string or None.
pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
    token_hash: &str,
    scopes: &str,
    expires_at: Option<&str>,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, scopes, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .bind(scopes)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Look up a token by its hash for authentication.
pub async fn find_by_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> sqlx::Result<Option<ApiTokenAuth>> {
    let row = sqlx::query(
        "SELECT id, user_id, scopes, expires_at, revoked_at \
         FROM api_tokens WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| ApiTokenAuth {
        id: r.get("id"),
        user_id: r.get("user_id"),
        scopes: r.get("scopes"),
        expires_at: r.get("expires_at"),
        revoked_at: r.get("revoked_at"),
    }))
}

/// Tokens for the management page, newest first.
pub async fn list_for_user(pool: &SqlitePool, user_id: &str) -> sqlx::Result<Vec<ApiTokenRow>> {
    let rows = sqlx::query(
        "SELECT id, name, scopes, created_at, last_used_at, expires_at, revoked_at \
         FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ApiTokenRow {
            id: r.get("id"),
            name: r.get("name"),
            scopes: r.get("scopes"),
            created_at: r.get("created_at"),
            last_used_at: r.get("last_used_at"),
            expires_at: r.get("expires_at"),
            revoked_at: r.get("revoked_at"),
        })
        .collect())
}

/// Revoke a token the caller owns. Sets `revoked_at` (the row is retained
/// for audit). Returns true when a still-active row was revoked.
pub async fn revoke(pool: &SqlitePool, id: i64, user_id: &str) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE api_tokens SET revoked_at = datetime('now') \
         WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Revoke every still-active token a user owns (LC-73: disabling a bot
/// revokes its tokens). Rows are retained for audit.
pub async fn revoke_all_for_user(pool: &SqlitePool, user_id: &str) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "UPDATE api_tokens SET revoked_at = datetime('now') \
         WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Best-effort `last_used_at` bump. Called from a detached task so it never
/// blocks the request.
pub async fn touch_last_used(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_tokens SET last_used_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
