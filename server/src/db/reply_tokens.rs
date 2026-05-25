//! LC-77-REPLY (#201) stage 1: side table for reply-by-email tokens.
//!
//! A per-mention notification email carries
//! `Reply-To: reply-<token>@<ingress-domain>`. The polled mailbox receives
//! the reply; the email-ingress resolver (stage 2) extracts the token from
//! the local part, calls [`resolve`], confirms `expires_at > now()`, and
//! hands off to the real-user post path with the resolved `user_id` +
//! `message_id`.
//!
//! Token security model:
//! - The plaintext token IS the credential. We store it plain because it
//!   leaks ONLY to a single recipient's verified email address (the
//!   notification email's only "To"), bounds itself via `expires_at`, and
//!   binds to one `(user_id, message_id)` pair so it cannot be replayed
//!   against other messages or other users.
//! - The bearer risk acknowledged in the operator docs: a forwarded
//!   notification email lets the recipient of the forward post as the
//!   original user until the token expires. Mitigations live above this
//!   module (per-user opt-in, 7-day TTL, per-user rate cap).

use rand::RngCore;
use sqlx::{Row, SqlitePool};

/// One reply-token row. Returned by `resolve`.
#[derive(Debug, Clone)]
pub struct ReplyTokenRow {
    pub user_id: String,
    pub message_id: i64,
    pub issued_at: String,
    pub expires_at: String,
}

/// Generate a fresh reply-token plaintext. 32 random bytes from `OsRng`,
/// base32-encoded with the RFC 4648 unpadded alphabet so the result is
/// safe as an email local part. ~52 chars, plus the `reply-` prefix
/// applied at the address-construction site keeps the local-part well
/// within RFC 5321's 64-char limit.
pub fn mint_token() -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut out = String::with_capacity(52);
    // 32 bytes = 256 bits = 51.2 base32 chars; we emit 52 chars (the last
    // includes 4 padding bits) without an `=` pad terminator.
    let mut bit_buf: u32 = 0;
    let mut bit_count: u32 = 0;
    for b in bytes {
        bit_buf = (bit_buf << 8) | b as u32;
        bit_count += 8;
        while bit_count >= 5 {
            bit_count -= 5;
            let idx = ((bit_buf >> bit_count) & 0x1F) as usize;
            out.push(ALPHABET[idx] as char);
        }
    }
    if bit_count > 0 {
        let idx = ((bit_buf << (5 - bit_count)) & 0x1F) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

/// Insert a fresh reply-token row. `expires_at` is an ISO-8601 string
/// (the same shape `datetime('now', '+7 days')` produces); the caller is
/// responsible for choosing the TTL.
pub async fn insert(
    pool: &SqlitePool,
    token: &str,
    user_id: &str,
    message_id: i64,
    expires_at: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO reply_tokens (token, user_id, message_id, expires_at) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(token)
    .bind(user_id)
    .bind(message_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Look up a token. Returns `None` for unknown tokens AND tokens whose
/// `message_id` has been deleted (cascade dropped the row). The caller
/// MUST check `row.expires_at` against the current time and drop expired
/// tokens; this function does not enforce expiry so an over-budget reply
/// can still be diagnosed by the caller with a specific `Expired` drop
/// reason rather than the catch-all "not found".
pub async fn resolve(pool: &SqlitePool, token: &str) -> sqlx::Result<Option<ReplyTokenRow>> {
    let row = sqlx::query(
        "SELECT user_id, message_id, issued_at, expires_at \
         FROM reply_tokens WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| ReplyTokenRow {
        user_id: r.get("user_id"),
        message_id: r.get("message_id"),
        issued_at: r.get("issued_at"),
        expires_at: r.get("expires_at"),
    }))
}

/// Look up a token AND tell the caller whether it has expired, in one
/// round trip. The bool is true when `expires_at < datetime('now')` per
/// SQLite's wall clock. Used by the email-ingress reply resolver so it
/// can drop with a specific `ReplyExpired` reason rather than the
/// catch-all `AddressNoMatch`.
pub async fn resolve_active_or_expired(
    pool: &SqlitePool,
    token: &str,
) -> sqlx::Result<Option<(ReplyTokenRow, bool)>> {
    let row = sqlx::query(
        "SELECT user_id, message_id, issued_at, expires_at, \
                (expires_at < datetime('now')) AS expired \
         FROM reply_tokens WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        let token_row = ReplyTokenRow {
            user_id: r.get("user_id"),
            message_id: r.get("message_id"),
            issued_at: r.get("issued_at"),
            expires_at: r.get("expires_at"),
        };
        let expired: bool = r.get::<i64, _>("expired") != 0;
        (token_row, expired)
    }))
}

/// Consume (delete) a reply token by its plaintext. Returns true if a
/// row was deleted. Used by the email-ingress reply path so a token can
/// only be used once (replay defense): even if a forwarded notification
/// email reaches a third party, the first successful POST consumes the
/// token and subsequent attempts drop as `AddressNoMatch`.
pub async fn consume(pool: &SqlitePool, token: &str) -> sqlx::Result<bool> {
    let res = sqlx::query("DELETE FROM reply_tokens WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Drop every row whose `expires_at` is in the past. Returns the count
/// of rows deleted for the orphan sweeper's log line.
pub async fn sweep_expired(pool: &SqlitePool) -> sqlx::Result<u64> {
    let res = sqlx::query("DELETE FROM reply_tokens WHERE expires_at < datetime('now')")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
