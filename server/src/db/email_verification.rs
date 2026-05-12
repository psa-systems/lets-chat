use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

/// 32 random bytes URL-base64-encoded (no padding) becomes the user-facing
/// token. The DB stores only its SHA-256 hex digest so a leaked snapshot of
/// `auth.db` cannot be used to forge verification URLs.
const TOKEN_BYTES: usize = 32;

/// Verification links live longer than password-reset links because users
/// commonly leave the welcome email unread for hours or days. A day is long
/// enough to remove most of the support burden without leaving stale tokens
/// active forever.
pub const VERIFY_TTL_MINUTES: i64 = 60 * 24;

/// Active row returned by [`find_active`]. `email` is the address the token
/// was issued for; the caller compares it against the user's *current*
/// `users.email` so a token issued before an in-flight email change cannot
/// silently verify the new address.
pub struct ActiveToken {
    pub user_id: String,
    pub email: String,
}

/// Create a single-use verification token for `user_id` pinned to `email`.
/// Returns the raw token string that should be emailed to the user.
pub async fn create_token(
    pool: &SqlitePool,
    user_id: &str,
    email: &str,
) -> Result<String, sqlx::Error> {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = base64_url_encode(&bytes);
    let hash = hash_token(&raw);
    let modifier = format!("+{VERIFY_TTL_MINUTES} minutes");
    sqlx::query(
        "INSERT INTO email_verification_tokens (token_hash, user_id, email, expires_at) \
         VALUES (?, ?, ?, datetime('now', ?))",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(email)
    .bind(&modifier)
    .execute(pool)
    .await?;
    Ok(raw)
}

/// Look up a token by its raw value. Returns the matching row only when it
/// exists, has not expired, and has not been consumed.
pub async fn find_active(
    pool: &SqlitePool,
    raw_token: &str,
) -> Result<Option<ActiveToken>, sqlx::Error> {
    let hash = hash_token(raw_token);
    let row = sqlx::query(
        "SELECT user_id, email FROM email_verification_tokens \
         WHERE token_hash = ? \
           AND used_at IS NULL \
           AND expires_at > datetime('now')",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| ActiveToken {
        user_id: r.get::<String, _>("user_id"),
        email: r.get::<String, _>("email"),
    }))
}

/// Mark a token as used. Single-use is enforced by the WHERE clause; a second
/// attempt updates zero rows.
pub async fn mark_used(pool: &SqlitePool, raw_token: &str) -> Result<u64, sqlx::Error> {
    let hash = hash_token(raw_token);
    let res = sqlx::query(
        "UPDATE email_verification_tokens SET used_at = datetime('now') \
         WHERE token_hash = ? AND used_at IS NULL",
    )
    .bind(&hash)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Invalidate every outstanding token for `user_id`. Called after a
/// successful verification, and whenever the user changes their email
/// address so an unsent token cannot be replayed against the old address.
pub async fn invalidate_all_for_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE email_verification_tokens SET used_at = datetime('now') \
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
