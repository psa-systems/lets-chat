//! SQLx helpers for the SSO tables (`sso_identities`, `sso_flows`).
//!
//! See docs/lets-chat/sso/05-schema-and-account-linking.md for the
//! state machine these helpers feed. Routes live in
//! `server/src/routes/sso.rs` (added in L5+); this module is pure DB.

use sqlx::{Row, SqlitePool};

/// One row in `sso_identities`. Returned by find/list helpers; the
/// callers in routes layer use this to render the Linked-Accounts
/// card on /settings/profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoIdentity {
    pub user_id: String,
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    /// `true` when the link was created via the `email_verified=true`
    /// auto-link path; `false` when password-confirmed or settings-linked.
    /// Drives the "Auto-linked on Aug 12" badge in the UI.
    pub auto_linked: bool,
    pub linked_at: String,
    pub last_seen_at: String,
}

/// One row in `sso_flows`. Returned ONCE per flow_id by
/// `consume_sso_flow`; the row is deleted in the same statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoFlow {
    pub csrf_state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: String,
    /// "sign_in" or "link" (matches the CHECK in the migration).
    pub kind: String,
    /// Non-null iff `kind = "link"`. The user who started the link
    /// flow from their already-signed-in session.
    pub user_id: Option<String>,
    /// Slug of the `sso_providers` row that started the flow. Added
    /// in migration 0018 so the callback handler knows which provider
    /// to exchange the code against.
    pub provider_id: String,
}

// ---------------------------------------------------------------------------
// sso_identities
// ---------------------------------------------------------------------------

/// Resolve a linked user by (issuer, subject). Returns the user_id
/// when a matching row exists AND the user is not banned. None for
/// either no link or a banned user. The banned filter keeps the
/// sign-in path from accidentally letting a banned identity back in
/// just because they linked SSO before the ban.
pub async fn find_user_by_sso(
    pool: &SqlitePool,
    issuer: &str,
    subject: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT u.id FROM users u \
         JOIN sso_identities s ON s.user_id = u.id \
         WHERE s.issuer = ? AND s.subject = ? AND u.is_banned = 0",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<String, _>("id")))
}

/// Upsert a link row: insert when (issuer, subject) is new; otherwise
/// touch `last_seen_at` and refresh the metadata `email`. The
/// `auto_linked` flag is sticky on update (we don't want to flip
/// auto -> manual just because the user signed in again).
pub async fn link_sso_identity(
    pool: &SqlitePool,
    user_id: &str,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
    auto_linked: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sso_identities (user_id, issuer, subject, email, auto_linked, linked_at, last_seen_at) \
         VALUES (?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
         ON CONFLICT (issuer, subject) DO UPDATE SET \
            last_seen_at = datetime('now'), \
            email = COALESCE(excluded.email, sso_identities.email)",
    )
    .bind(user_id)
    .bind(issuer)
    .bind(subject)
    .bind(email)
    .bind(if auto_linked { 1 } else { 0 })
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete every sso_identities row for the user. v1 has at most one
/// row per user (single IdP); BYO-SSO grows this to one-per-provider
/// and adds a parallel `unlink_for_provider` helper. Refuses (via the
/// route layer, not here) when the user has no local password — this
/// helper is unconditional at the DB level.
pub async fn unlink_sso_identity(pool: &SqlitePool, user_id: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sso_identities WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// List a user's linked identities. v1 returns at most 1 row; BYO-SSO
/// returns up to N. Ordered by linked_at so the oldest appears first.
pub async fn list_sso_identities_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<SsoIdentity>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT user_id, issuer, subject, email, auto_linked, linked_at, last_seen_at \
         FROM sso_identities WHERE user_id = ? ORDER BY linked_at ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SsoIdentity {
            user_id: r.get("user_id"),
            issuer: r.get("issuer"),
            subject: r.get("subject"),
            email: r.get("email"),
            auto_linked: r.get::<i64, _>("auto_linked") != 0,
            linked_at: r.get("linked_at"),
            last_seen_at: r.get("last_seen_at"),
        })
        .collect())
}

/// True when the user's `password_hash` is non-null + non-empty.
/// Routes use this to refuse `unlink_sso_identity` when the user
/// would otherwise lock themselves out.
pub async fn user_has_password(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .and_then(|r| r.get::<Option<String>, _>("password_hash"))
        .map(|s| !s.is_empty())
        .unwrap_or(false))
}

/// Arguments for auto-provisioning a fresh user from SSO claims.
/// Borrowed so the caller doesn't have to clone for the FFI.
pub struct CreateUserFromSso<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    pub email: Option<&'a str>,
    /// IdP-supplied `preferred_username` claim. May collide with an
    /// existing local username; the helper resolves by suffixing
    /// `-2`, `-3`, ... up to a cap.
    pub preferred_username: Option<&'a str>,
    /// IdP-supplied `name` claim. Stored as display_name.
    pub display_name: Option<&'a str>,
}

/// Atomic two-table insert: new `users` row (NULL password_hash) +
/// matching `sso_identities` row. Resolves username collisions by
/// suffixing `-2`, `-3`, ... up to 50 attempts; returns `Other` after
/// that as a defensive bail.
///
/// Wrapped in `BEGIN IMMEDIATE ... COMMIT` so a concurrent signup with
/// the same preferred_username can't race past the uniqueness check.
pub async fn create_user_from_sso(
    pool: &SqlitePool,
    args: CreateUserFromSso<'_>,
) -> Result<String, sqlx::Error> {
    const MAX_COLLISION_ATTEMPTS: i32 = 50;

    let base_username = args
        .preferred_username
        .map(sanitize_username)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // Fall back to the subject hashed-into-a-username when
            // the IdP gave us nothing usable. This is rare; mokosh
            // always emits preferred_username.
            let mut s = String::from("sso-");
            for c in args.subject.chars().take(8) {
                if c.is_ascii_alphanumeric() {
                    s.push(c);
                }
            }
            s
        });

    let user_id = uuid::Uuid::new_v4().to_string();
    let mut tx = pool.begin().await?;

    // Find a non-colliding username. The `users.username` index is
    // UNIQUE COLLATE NOCASE; this loop matches that comparator.
    let mut chosen = base_username.clone();
    let mut attempt = 1i32;
    loop {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM users WHERE username = ? COLLATE NOCASE LIMIT 1")
                .bind(&chosen)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            break;
        }
        attempt += 1;
        if attempt > MAX_COLLISION_ATTEMPTS {
            return Err(sqlx::Error::Protocol(format!(
                "create_user_from_sso: {MAX_COLLISION_ATTEMPTS} username-collision attempts \
                 exhausted starting from {base_username:?}"
            )));
        }
        chosen = format!("{base_username}-{attempt}");
    }

    // password_hash NULL so the user has no local-password sign-in
    // path (per docs/lets-chat/sso/02 §3). Email is set when the IdP
    // supplied it; otherwise NULL.
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, email, last_active_at) \
         VALUES (?, ?, ?, NULL, ?, datetime('now'))",
    )
    .bind(&user_id)
    .bind(&chosen)
    .bind(args.display_name)
    .bind(args.email)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO sso_identities (user_id, issuer, subject, email, auto_linked, linked_at, last_seen_at) \
         VALUES (?, ?, ?, ?, 1, datetime('now'), datetime('now'))",
    )
    .bind(&user_id)
    .bind(args.issuer)
    .bind(args.subject)
    .bind(args.email)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(user_id)
}

/// Map an IdP `preferred_username` claim to a string we'd accept in
/// our `users.username` column. lets-chat accepts a narrow character
/// set; we filter to that set and drop anything else. Empty result
/// means the caller should fall back.
fn sanitize_username(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(32)
        .collect()
}

// ---------------------------------------------------------------------------
// sso_flows
// ---------------------------------------------------------------------------

/// Insert a flow row keyed by `flow_id`. The flow expires `ttl_seconds`
/// after insertion (10 minutes is the standard window per doc 02 §12).
#[allow(clippy::too_many_arguments)]
pub async fn insert_sso_flow(
    pool: &SqlitePool,
    flow_id: &str,
    csrf_state: &str,
    nonce: &str,
    pkce_verifier: &str,
    return_to: &str,
    kind: &str,
    user_id: Option<&str>,
    provider_id: &str,
    ttl_seconds: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sso_flows \
            (flow_id, csrf_state, nonce, pkce_verifier, return_to, kind, user_id, provider_id, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now', ? || ' seconds'))",
    )
    .bind(flow_id)
    .bind(csrf_state)
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(return_to)
    .bind(kind)
    .bind(user_id)
    .bind(provider_id)
    .bind(ttl_seconds.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// One-shot lookup + delete. Returns Some when the row exists AND has
/// not expired; otherwise None. Implemented via DELETE ... RETURNING
/// so the row is consumed atomically (can't be replayed).
pub async fn consume_sso_flow(
    pool: &SqlitePool,
    flow_id: &str,
) -> Result<Option<SsoFlow>, sqlx::Error> {
    let row = sqlx::query(
        "DELETE FROM sso_flows WHERE flow_id = ? AND expires_at > datetime('now') \
         RETURNING csrf_state, nonce, pkce_verifier, return_to, kind, user_id, provider_id",
    )
    .bind(flow_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| SsoFlow {
        csrf_state: r.get("csrf_state"),
        nonce: r.get("nonce"),
        pkce_verifier: r.get("pkce_verifier"),
        return_to: r.get("return_to"),
        kind: r.get("kind"),
        provider_id: r.get("provider_id"),
        user_id: r.get("user_id"),
    }))
}

/// Drop every expired `sso_flows` row. Called by the background task
/// every 5 minutes. Returns the number of rows pruned for tracing.
pub async fn prune_expired_sso_flows(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sso_flows WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    //! Unit tests for the helpers. The integration test binary
    //! `server/tests/db_sso.rs` exercises the same functions end-to-end
    //! with a fresh in-memory DB; here we only assert the pure helpers
    //! (`sanitize_username`).

    use super::*;

    #[test]
    fn sanitize_username_keeps_acceptable_chars() {
        assert_eq!(sanitize_username("alice"), "alice");
        assert_eq!(sanitize_username("alice_smith"), "alice_smith");
        assert_eq!(sanitize_username("alice-smith"), "alice-smith");
        assert_eq!(sanitize_username("alice.smith"), "alice.smith");
    }

    #[test]
    fn sanitize_username_drops_disallowed_chars() {
        assert_eq!(sanitize_username("alice@example.com"), "aliceexample.com");
        assert_eq!(sanitize_username("Bob Smith"), "BobSmith");
        assert_eq!(sanitize_username("user/name"), "username");
    }

    #[test]
    fn sanitize_username_caps_length() {
        let long: String = "a".repeat(200);
        let sanitized = sanitize_username(&long);
        assert_eq!(sanitized.len(), 32);
    }
}
