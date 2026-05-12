use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

/// 32 random bytes URL-base64-encoded (no padding) becomes the user-facing
/// token. The DB stores only its SHA-256 hex digest so a leaked snapshot of
/// `auth.db` cannot be used to forge reset URLs.
const TOKEN_BYTES: usize = 32;

pub const RESET_TTL_MINUTES: i64 = 60;

/// Create a single-use reset token for `user_id` and return the raw token
/// string that should be emailed to the user. The matching hash is stored
/// in `password_reset_tokens` with the configured TTL.
pub async fn create_token(pool: &SqlitePool, user_id: &str) -> Result<String, sqlx::Error> {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = base64_url_encode(&bytes);
    let hash = hash_token(&raw);
    let modifier = format!("+{RESET_TTL_MINUTES} minutes");
    sqlx::query(
        "INSERT INTO password_reset_tokens (token_hash, user_id, expires_at) \
         VALUES (?, ?, datetime('now', ?))",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(&modifier)
    .execute(pool)
    .await?;
    Ok(raw)
}

/// Look up a token by its raw value. Returns the matching `user_id` only
/// when the row exists, has not expired, and has not been consumed.
pub async fn find_active_user_id(
    pool: &SqlitePool,
    raw_token: &str,
) -> Result<Option<String>, sqlx::Error> {
    let hash = hash_token(raw_token);
    let row = sqlx::query(
        "SELECT user_id FROM password_reset_tokens \
         WHERE token_hash = ? \
           AND used_at IS NULL \
           AND expires_at > datetime('now')",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<String, _>("user_id")))
}

/// Mark a token as used. The single-use guard is enforced by the WHERE clause
/// (`used_at IS NULL`); a second attempt updates zero rows so the caller can
/// detect double-spend by inspecting the rows-affected count.
pub async fn mark_used(pool: &SqlitePool, raw_token: &str) -> Result<u64, sqlx::Error> {
    let hash = hash_token(raw_token);
    let res = sqlx::query(
        "UPDATE password_reset_tokens SET used_at = datetime('now') \
         WHERE token_hash = ? AND used_at IS NULL",
    )
    .bind(&hash)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Invalidate every outstanding (not-yet-used, not-yet-expired) token for
/// `user_id`. Called after a successful reset so any other pending links
/// in the user's inbox stop working immediately.
pub async fn invalidate_all_for_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE password_reset_tokens SET used_at = datetime('now') \
         WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    hex_encode(&digest)
}

fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
