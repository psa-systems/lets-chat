//! LC-77: per-room email-ingress inboxes. The email-shaped sibling of LC-74
//! incoming webhooks (`crate::db::webhooks`). An external sender mails the
//! polled mailbox at `<token>@<ingress-domain>`; the IMAP poll loop in
//! commit 3 will resolve the token to a row in this table, hash-match the
//! secret, and post the body as a synthetic "Email" actor message.
//!
//! Only the HMAC of an inbox secret is stored (keyed by the server secret;
//! same `crate::auth::hash_api_token` shape webhooks use). The plaintext
//! token lives only in the address shown ONCE at inbox creation.

use sqlx::{Row, SqlitePool};

/// Display identity for rendering an email-ingress-authored message.
/// Mirrors `crate::db::webhooks::WebhookIdentity` field-for-field so the
/// render layer in commit 3 can drop in alongside the webhook arm.
#[derive(Debug, Clone)]
pub struct EmailInboxIdentity {
    pub name: String,
    pub avatar_url: Option<String>,
}

/// Display identity for an email-inbox id (rendering an email-ingress
/// message). `None` if the inbox row was hard-deleted; with the
/// `ON DELETE SET NULL` FK on `messages.email_inbox_id`, the caller
/// reaches this only when the column is set, but the inbox row may still
/// be missing in race-with-delete scenarios.
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
