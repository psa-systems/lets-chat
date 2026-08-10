use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

use crate::models::invite::InviteCode;
use crate::models::user::UserRecord;

/// Legacy test-only helper: writes a `users` row with a synthesized unique
/// `bunyip_sub` placeholder so the LC-22 unique constraint stays satisfied.
/// Production code paths use `create_user_from_bunyip` with a real Bunyip sub.
///
/// LC-22 cutover: the password-path `post_register` handler is gone. This
/// helper survives because ~20 test files construct users via this signature;
/// rewriting every call site is mechanical follow-up work. The
/// `password_hash` argument is ignored at runtime (the column is set to an
/// empty sentinel) but kept in the signature so test files compile unchanged.
pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    _password_hash: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    // Synthesize a unique bunyip_sub placeholder so multiple test users in
    // one pool do not collide on the UNIQUE constraint added in 0031.
    let placeholder = format!("test-{id}");
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, bunyip_sub, last_active_at) \
         VALUES (?, ?, '', ?, datetime('now'))",
    )
    .bind(&id)
    .bind(username)
    .bind(&placeholder)
    .execute(pool)
    .await?;
    Ok(id)
}

/// LC-73: create a bot user. `is_bot = 1`, empty password hash (login refuses
/// it: bots authenticate only via API tokens). Returns the new user id.
pub async fn create_bot(pool: &SqlitePool, username: &str) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    // Bots get a synthesized placeholder bunyip_sub for the same reason
    // create_user does (UNIQUE constraint, no real Bunyip identity).
    let placeholder = format!("bot-{id}");
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, bunyip_sub, is_bot, last_active_at) \
         VALUES (?, ?, '', ?, 1, datetime('now'))",
    )
    .bind(&id)
    .bind(username)
    .bind(&placeholder)
    .execute(pool)
    .await?;
    Ok(id)
}

/// LC-73: bot users, newest first.
pub async fn list_bots(pool: &SqlitePool) -> Result<Vec<UserRecord>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at, read_receipts_enabled, \
         bio, avatar_ext, status, custom_status, last_active_at, is_profile_public, \
         notify_browser_enabled, notify_sound_enabled, notify_push_enabled, \
         notify_email_digest_enabled, notify_login_alerts_enabled, \
         notify_email_activity_enabled, \
         last_ws_seen_at, last_digest_sent_at, \
         dnd_schedule_json, dnd_paused_until, email, \
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes, is_bot, locale, theme_mode, theme_palette, theme_scale, home_landing, density, \
         pronouns, profile_links, timezone \
         FROM users WHERE is_bot = 1 ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_user_record).collect())
}

pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at, read_receipts_enabled, \
         bio, avatar_ext, status, custom_status, last_active_at, is_profile_public, \
         notify_browser_enabled, notify_sound_enabled, notify_push_enabled, \
         notify_email_digest_enabled, notify_login_alerts_enabled, \
         notify_email_activity_enabled, \
         last_ws_seen_at, last_digest_sent_at, \
         dnd_schedule_json, dnd_paused_until, email, \
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes, is_bot, locale, theme_mode, theme_palette, theme_scale, home_landing, density, \
         pronouns, profile_links, timezone \
         FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_user_record))
}

pub async fn find_user_by_id(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at, read_receipts_enabled, \
         bio, avatar_ext, status, custom_status, last_active_at, is_profile_public, \
         notify_browser_enabled, notify_sound_enabled, notify_push_enabled, \
         notify_email_digest_enabled, notify_login_alerts_enabled, \
         notify_email_activity_enabled, \
         last_ws_seen_at, last_digest_sent_at, \
         dnd_schedule_json, dnd_paused_until, email, \
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes, is_bot, locale, theme_mode, theme_palette, theme_scale, home_landing, density, \
         pronouns, profile_links, timezone \
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_user_record))
}

fn row_to_user_record(r: sqlx::sqlite::SqliteRow) -> UserRecord {
    UserRecord {
        id: r.get("id"),
        username: r.get("username"),
        display_name: r.get("display_name"),
        password_hash: r.get("password_hash"),
        role: r.get("role"),
        is_banned: r.get("is_banned"),
        ban_reason: r.get("ban_reason"),
        banned_until: r.get("banned_until"),
        is_muted: r.get("is_muted"),
        muted_until: r.get("muted_until"),
        mute_reason: r.get("mute_reason"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        read_receipts_enabled: r.get("read_receipts_enabled"),
        bio: r.get("bio"),
        avatar_ext: r.get("avatar_ext"),
        status: r.get("status"),
        custom_status: r.get("custom_status"),
        last_active_at: r.get("last_active_at"),
        is_profile_public: r.get("is_profile_public"),
        notify_browser_enabled: r.get::<i64, _>("notify_browser_enabled") != 0,
        notify_sound_enabled: r.get::<i64, _>("notify_sound_enabled") != 0,
        notify_push_enabled: r.get::<i64, _>("notify_push_enabled") != 0,
        notify_email_digest_enabled: r.get::<i64, _>("notify_email_digest_enabled") != 0,
        notify_login_alerts_enabled: r.get::<i64, _>("notify_login_alerts_enabled") != 0,
        notify_email_activity_enabled: r.get::<i64, _>("notify_email_activity_enabled") != 0,
        last_ws_seen_at: r.get("last_ws_seen_at"),
        last_digest_sent_at: r.get("last_digest_sent_at"),
        dnd_schedule_json: r.get("dnd_schedule_json"),
        dnd_paused_until: r.get("dnd_paused_until"),
        email: r.get("email"),
        totp_secret_encrypted: r.get("totp_secret_encrypted"),
        totp_nonce: r.get("totp_nonce"),
        totp_enabled: r.get("totp_enabled"),
        totp_recovery_hashes: r.get("totp_recovery_hashes"),
        is_bot: r.get::<i64, _>("is_bot") != 0,
        locale: r.get("locale"),
        theme_mode: r.get("theme_mode"),
        theme_palette: r.get("theme_palette"),
        theme_scale: r.get("theme_scale"),
        home_landing: r.get("home_landing"),
        density: r.get("density"),
        pronouns: r.get("pronouns"),
        profile_links: r.get("profile_links"),
        timezone: r.get("timezone"),
    }
}

/// Allowed values for the `users.status` column.
///
/// `idle` and `away` both render as "Away" in the UI but differ in semantics:
/// - `idle` is auto-applied by the background tick when `last_active_at`
///   crosses the threshold, and is cleared automatically by the next HTTP
///   request (see `touch_user_activity`).
/// - `away` is manually selected from the status picker and is sticky: HTTP
///   activity does not clear it. The user must change it back themselves.
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_IDLE: &str = "idle";
pub const STATUS_AWAY: &str = "away";
pub const STATUS_DND: &str = "dnd";

/// LC-78: role tier for a protocol-bridge bot user. Strictly narrower than
/// `user`: handlers outside the /api/v1/bridges/* surface use
/// `ApiAuth::require_not_bridge` to reject this role, so the daemon's
/// blast radius is exactly bridge-post + heartbeat even if an operator
/// mistakenly grants it `messages:write` / `messages:read` / `rooms:read`.
/// The role gate is defense in depth on top of scope gating + the LC-73
/// cookie-login-rejects-bots posture.
pub const ROLE_BRIDGE: &str = "bridge";
pub const MAX_CUSTOM_STATUS_CHARS: usize = 50;

pub fn is_valid_status(s: &str) -> bool {
    matches!(s, STATUS_ACTIVE | STATUS_IDLE | STATUS_AWAY | STATUS_DND)
}

#[derive(Debug, thiserror::Error)]
pub enum SetStatusError {
    #[error("invalid status value")]
    InvalidStatus,
    #[error("custom status exceeds {0} characters")]
    CustomTooLong(usize),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// LC-319: persist a user's status + custom text, optionally scheduling the
/// custom text to auto-clear.
///
/// `expires_modifier` is a SQLite relative-time modifier (e.g. `"+1 hours"`)
/// applied as `datetime('now', ?)`. It MUST come from the route's fixed
/// allowlist (`routes::status::expiry_modifier`), never raw user input, since
/// it is interpolated into the time function. `None` clears any existing
/// expiry. An expiry without custom text is meaningless, so the caller is
/// expected to pass `None` whenever `custom` is `None`; the sweep
/// (`clear_expired_custom_statuses`) only ever nulls a non-null expiry, so a
/// stray expiry on an empty status is harmless either way.
pub async fn set_user_status(
    pool: &SqlitePool,
    user_id: &str,
    status: &str,
    custom: Option<&str>,
    expires_modifier: Option<&str>,
) -> Result<(), SetStatusError> {
    if !is_valid_status(status) {
        return Err(SetStatusError::InvalidStatus);
    }
    if let Some(c) = custom {
        if c.chars().count() > MAX_CUSTOM_STATUS_CHARS {
            return Err(SetStatusError::CustomTooLong(MAX_CUSTOM_STATUS_CHARS));
        }
    }
    let now_clause = if status == STATUS_ACTIVE {
        ", last_active_at = datetime('now')"
    } else {
        ""
    };
    // The CASE keeps this a single statement: a NULL modifier nulls the expiry
    // column; a present modifier resolves to an absolute timestamp relative to
    // now. The modifier is bound twice (the IS NULL guard and the datetime arg)
    // so the present/absent branch is chosen at SQL eval time.
    let sql = format!(
        "UPDATE users SET status = ?, custom_status = ?, \
         custom_status_expires_at = CASE WHEN ? IS NULL THEN NULL ELSE datetime('now', ?) END, \
         updated_at = datetime('now'){now_clause} WHERE id = ?"
    );
    sqlx::query(&sql)
        .bind(status)
        .bind(custom)
        .bind(expires_modifier)
        .bind(expires_modifier)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-319: the absolute ISO expiry of a user's custom status, if one is
/// scheduled. Read only by the status picker to render the "auto-clears" hint;
/// kept out of `row_to_user_record` so the shared `User`/`UserRecord` model and
/// its read sites stay untouched.
pub async fn get_custom_status_expiry(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT custom_status_expires_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("custom_status_expires_at")))
}

/// LC-319: clear every custom status whose `custom_status_expires_at` has
/// passed, returning `(user_id, status)` for each so the caller can broadcast a
/// `UserStatusChanged` with the (unchanged) presence value. Only the custom
/// text + expiry are nulled; the presence `status` is deliberately untouched.
/// Mirrors `mark_idle_users`: a single RETURNING UPDATE, no read-path writes.
pub async fn clear_expired_custom_statuses(
    pool: &SqlitePool,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "UPDATE users SET custom_status = NULL, custom_status_expires_at = NULL, \
         updated_at = datetime('now') \
         WHERE custom_status_expires_at IS NOT NULL \
         AND custom_status_expires_at <= datetime('now') \
         RETURNING id, status",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("id"), r.get::<String, _>("status")))
        .collect())
}

/// Refresh `last_active_at`. If the user was `'idle'`, promote them back to
/// `'active'`. DND and manual `'away'` are sticky: bumps the timestamp so the
/// idle clock restarts once they leave that state, but never overwrites the
/// status. Returns `true` only when the call actually flipped idle->active so
/// the caller can broadcast.
///
/// Implemented as two single-statement updates rather than a SELECT-then-
/// UPDATE transaction: under chat load this runs on every WebSocket message
/// and every room visit, so holding a write lock for the duration of a
/// round-trip multiplies contention and produces `SQLITE_BUSY` even with
/// WAL + busy_timeout enabled. The first statement only matches idle rows,
/// never DND or away, so the sticky-state guarantees are preserved.
pub async fn touch_user_activity(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let flipped = sqlx::query(
        "UPDATE users \
         SET status = 'active', \
             last_active_at = datetime('now'), \
             updated_at = datetime('now') \
         WHERE id = ? AND status = 'idle'",
    )
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0;

    if !flipped {
        // The user was already active or in DND; just refresh the activity
        // timestamp without touching `status`. A missing user row is a no-op.
        sqlx::query("UPDATE users SET last_active_at = datetime('now') WHERE id = ?")
            .bind(user_id)
            .execute(pool)
            .await?;
    }

    Ok(flipped)
}

/// Bump `users.last_ws_seen_at` to `now()` for `user_id`. Called from the
/// WebSocket path to record that the in-app notification surface had a chance
/// to fire for this user (connection-open or an outbound `Mentioned` frame
/// reaching the client). The digest "missed" predicate consults this column
/// alongside `last_active_at` to decide whether to email about a mention.
///
/// Idle status (see `mark_idle_users`) deliberately does NOT consult this
/// column. The two timestamps serve different purposes:
/// - `last_active_at` is "the user interacted with the app via HTTP."
/// - `last_ws_seen_at` is "the user's app was alive enough to surface a ping."
///
/// Errors are logged at warn but not propagated: the WS hot path should not
/// fail because a side-effect DB write hit a snag.
pub async fn bump_last_ws_seen(pool: &SqlitePool, user_id: &str) {
    if let Err(e) = sqlx::query("UPDATE users SET last_ws_seen_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await
    {
        tracing::warn!(error = %e, user_id = %user_id, "bump_last_ws_seen failed");
    }
}

/// A single row in the eligibility query result. The tick reads these
/// off and then runs per-user "what did they miss" queries against the
/// chat pool, so we project only the fields the tick actually needs.
#[derive(Debug, Clone)]
pub struct DigestCandidate {
    pub id: String,
    pub username: String,
    pub email: String,
    /// MAX(last_active_at, COALESCE(last_ws_seen_at, '')). The tick uses
    /// it as the lower bound on "missed" mention/DM created_at, AND as
    /// the comparison point for `last_digest_sent_at`.
    pub activity_floor: String,
    /// LC-88: DND schedule + manual pause, carried so the digest tick can
    /// hold a send while the recipient is in a quiet window.
    pub dnd_schedule_json: Option<String>,
    pub dnd_paused_until: Option<String>,
}

/// Find users currently eligible for an email-digest send. A user is
/// eligible iff:
/// - `notify_email_digest_enabled = 1`
/// - they have a non-empty email address
/// - both `last_active_at` and `last_ws_seen_at` are older than
///   `quiet_period_secs` (no HTTP activity AND no WS pings recently)
/// - either no digest has ever been sent, or the last digest predates
///   the user's most recent activity (i.e. they came back online and
///   went offline again; "one digest per offline session")
///
/// The `MAX(a, COALESCE(b, ''))` shape works because lets-chat stores
/// timestamps as ISO 8601 strings, which sort lexicographically the
/// same as chronologically. The `COALESCE` lets users who have never
/// connected via WS still qualify (their floor is `last_active_at`
/// alone).
pub async fn find_digest_candidates(
    pool: &SqlitePool,
    quiet_period_secs: i64,
) -> Result<Vec<DigestCandidate>, sqlx::Error> {
    let quiet_modifier = format!("-{quiet_period_secs} seconds");
    let rows = sqlx::query(
        "SELECT id, username, email, dnd_schedule_json, dnd_paused_until, \
                MAX(last_active_at, COALESCE(last_ws_seen_at, '')) AS activity_floor \
           FROM users \
          WHERE notify_email_digest_enabled = 1 \
            AND email IS NOT NULL AND email <> '' \
            AND MAX(last_active_at, COALESCE(last_ws_seen_at, '')) < datetime('now', ?) \
            AND (last_digest_sent_at IS NULL \
                 OR last_digest_sent_at < MAX(last_active_at, COALESCE(last_ws_seen_at, '')))",
    )
    .bind(quiet_modifier)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DigestCandidate {
            id: r.get("id"),
            username: r.get("username"),
            email: r.get("email"),
            activity_floor: r.get("activity_floor"),
            dnd_schedule_json: r.get("dnd_schedule_json"),
            dnd_paused_until: r.get("dnd_paused_until"),
        })
        .collect())
}

/// Mark a digest as just sent for `user_id`. The digest tick gates eligibility
/// with `last_digest_sent_at < MAX(last_active_at, last_ws_seen_at)`, so this
/// timestamp self-resets the moment the user comes back online and bumps
/// either activity column. One digest per offline session.
pub async fn set_last_digest_sent_at(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET last_digest_sent_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-671: real (non-bot) users who are due for a weekly recap - active in the
/// last 7 days and not recapped in the last 7 days. Returns user ids; the caller
/// fetches each one's weekly figures from the chat db and skips those with
/// nothing to celebrate.
pub async fn weekly_recap_candidates(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM users \
          WHERE is_bot = 0 \
            AND last_active_at >= datetime('now', '-7 days') \
            AND (last_weekly_recap_at IS NULL \
                 OR last_weekly_recap_at < datetime('now', '-7 days'))",
    )
    .fetch_all(pool)
    .await
}

/// LC-671: mark a user's weekly recap as just handled (dedupe marker). Bumped on
/// every evaluation, whether or not a recap was actually sent, so a user with a
/// quiet week is not re-evaluated until the next 7-day window.
pub async fn set_last_weekly_recap_at(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET last_weekly_recap_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Flip rows whose `status = 'active'` and whose `last_active_at` is older
/// than `threshold_seconds` to `'idle'`. Returns the IDs that flipped.
///
/// Idle status reflects HTTP-request activity only (`last_active_at`). The
/// separate `last_ws_seen_at` column is the digest's "in-app surface was
/// alive" signal; it deliberately does NOT participate in idle-flip, so a
/// user with a tab open in a busy room continues to flip to idle after
/// `threshold_seconds` of no HTTP interaction.
pub async fn mark_idle_users(
    pool: &SqlitePool,
    threshold_seconds: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let modifier = format!("-{threshold_seconds} seconds");
    let rows = sqlx::query(
        "UPDATE users SET status = 'idle', updated_at = datetime('now') \
         WHERE status = 'active' AND last_active_at < datetime('now', ?) \
         RETURNING id",
    )
    .bind(modifier)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>("id")).collect())
}

pub async fn is_user_dnd(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT status FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .map(|r| r.get::<String, _>("status") == STATUS_DND)
        .unwrap_or(false))
}

pub async fn count_users(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) as count FROM users")
        .fetch_one(pool)
        .await?;
    Ok(row.get("count"))
}

pub async fn set_user_role(
    pool: &SqlitePool,
    user_id: &str,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET role = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(role)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-352: true if an active (role=admin, not banned) admin OTHER than
/// `excluding_user_id` exists. Used to refuse the demote / ban / delete that
/// would otherwise drop the active-admin count to zero and lock everyone out
/// of the admin surface.
pub async fn other_active_admin_exists(
    pool: &SqlitePool,
    excluding_user_id: &str,
) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE role = 'admin' AND is_banned = 0 AND id != ?)",
    )
    .bind(excluding_user_id)
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// Issue a session token for `user_id` with no captured origin metadata.
/// Production code should prefer `create_session_with_origin` so the
/// settings sessions list can show a meaningful row; this no-origin variant
/// stays for tests and legacy paths.
pub async fn create_session(pool: &SqlitePool, user_id: &str) -> Result<String, sqlx::Error> {
    create_session_with_origin(pool, user_id, None, None).await
}

pub async fn create_session_with_origin(
    pool: &SqlitePool,
    user_id: &str,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<String, sqlx::Error> {
    use rand::Rng;
    // LC-155: explicit OS CSPRNG for the session token (see generate_api_token).
    let token: String = rand::rngs::OsRng
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    // LC-514: store SHA-256(token) as `sessions.id` so a read-only DB
    // compromise yields no usable cookies. The plaintext is returned to
    // the caller and lands in the browser's session cookie; every
    // subsequent lookup re-hashes the presented value and matches by
    // the hash. See `hash_session_token` for the in-flight invariant.
    let hashed = hash_session_token(&token);
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at, user_agent, ip, last_seen_at) \
         VALUES (?, ?, datetime('now', '+30 days'), ?, ?, datetime('now'))",
    )
    .bind(&hashed)
    .bind(user_id)
    .bind(user_agent)
    .bind(ip)
    .execute(pool)
    .await?;

    Ok(token)
}

/// LC-514: SHA-256 hex of the raw cookie token. Stored as `sessions.id`
/// so a DB compromise yields no usable sessions. Lookups MUST re-hash
/// the presented cookie value before comparing.
///
/// Idempotent on an already-hashed input: a 64-char lowercase hex string
/// re-hashes to a different value, which is fine - the lookup site only
/// ever sees raw cookie tokens from the browser (never re-hashes a hash).
/// The one-time backfill in `backfill_sessions_hashed_at_rest` skips rows
/// that already look like the SHA-256 hex shape via `is_sha256_hex` so
/// existing valid sessions are migrated exactly once.
pub fn hash_session_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let bytes = hasher.finalize();
    hex::encode(bytes)
}

/// LC-514: whether `s` matches the SHA-256 hex shape (64 lowercase hex
/// chars). Used by the in-place backfill to skip rows that have already
/// been re-hashed by a previous server start.
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// LC-514: in-place re-hash of pre-migration session rows.
///
/// Runs at most once per database (gated by the
/// `sessions_hash_migration_marker` table). Walks every row whose `id`
/// does NOT already look like a SHA-256 hex string and UPDATEs `id` to
/// `SHA-256(id)`. The cookie in flight still carries the original
/// plaintext, so a freshly-migrated DB matches incoming cookies via the
/// new lookup logic. New sessions minted post-migration always store
/// the hash via `create_session_with_origin`.
///
/// Idempotent: re-runs on a fully-hashed table do nothing because every
/// row passes `is_sha256_hex`. The marker also flips to `completed=1`
/// after the first successful run so subsequent starts skip the full
/// scan entirely.
pub async fn backfill_sessions_hashed_at_rest(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    use sqlx::Row;
    let already: Option<(i64,)> =
        sqlx::query_as("SELECT completed FROM sessions_hash_migration_marker WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    if matches!(already, Some((1,))) {
        return Ok(());
    }

    let rows = sqlx::query("SELECT id FROM sessions")
        .fetch_all(pool)
        .await?;
    let mut migrated: u64 = 0;
    for row in rows {
        let id: String = row.get("id");
        if is_sha256_hex(&id) {
            continue;
        }
        let hashed = hash_session_token(&id);
        let res = sqlx::query("UPDATE sessions SET id = ? WHERE id = ?")
            .bind(&hashed)
            .bind(&id)
            .execute(pool)
            .await?;
        migrated += res.rows_affected();
    }

    sqlx::query("UPDATE sessions_hash_migration_marker SET completed = 1 WHERE id = 1")
        .execute(pool)
        .await?;

    tracing::info!(
        target: "auth",
        migrated,
        "LC-514: sessions table re-hashed at rest"
    );
    Ok(())
}

/// Bump `last_seen_at` for a live session. Called from the auth middleware on
/// every authed request; throttled by `LAST_SEEN_REFRESH_SECONDS` so the write
/// rate stays well below the read rate.
///
/// LC-514: `session_id` is the raw cookie value; hash it to find the row.
pub async fn touch_session_last_seen(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    let hashed = hash_session_token(session_id);
    sqlx::query("UPDATE sessions SET last_seen_at = datetime('now') WHERE id = ?")
        .bind(&hashed)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: Option<String>,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

/// List a user's live sessions (expiry-filtered), newest activity first.
pub async fn list_sessions_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, created_at, expires_at, last_seen_at, user_agent, ip \
         FROM sessions \
         WHERE user_id = ? AND expires_at > datetime('now') \
         ORDER BY COALESCE(last_seen_at, created_at) DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SessionRow {
            id: r.get("id"),
            created_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
            last_seen_at: r.try_get("last_seen_at").ok(),
            user_agent: r.try_get("user_agent").ok(),
            ip: r.try_get("ip").ok(),
        })
        .collect())
}

/// Delete a single session, scoped to the owning user. Returns `true` if a
/// row was actually removed. The user-scope guard prevents one user from
/// revoking another user's session by guessing or replaying a session ID.
/// LC-514: caller may pass either the raw cookie or the already-hashed
/// row id (e.g. from `list_sessions_for_user`, which returns the stored
/// `id` which is itself the hash). We always re-hash if the value looks
/// like a raw token (NOT already a 64-char hex); already-hashed inputs
/// are passed through. This keeps the settings UI's "Sign out this
/// session" button working without changing the visible shape.
fn lookup_session_id(presented: &str) -> String {
    if is_sha256_hex(presented) {
        presented.to_string()
    } else {
        hash_session_token(presented)
    }
}

pub async fn delete_session_for_user(
    pool: &SqlitePool,
    session_id: &str,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(lookup_session_id(session_id))
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn get_user_by_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT u.id, u.username, u.display_name, u.password_hash, u.role, \
         u.is_banned, u.ban_reason, u.banned_until, \
         u.is_muted, u.muted_until, u.mute_reason, \
         u.created_at, u.updated_at, u.read_receipts_enabled, \
         u.bio, u.avatar_ext, u.status, u.custom_status, u.last_active_at, u.is_profile_public, \
         u.notify_browser_enabled, u.notify_sound_enabled, u.notify_push_enabled, \
         u.notify_email_digest_enabled, u.notify_login_alerts_enabled, \
         u.notify_email_activity_enabled, \
         u.last_ws_seen_at, u.last_digest_sent_at, \
         u.dnd_schedule_json, u.dnd_paused_until, u.email, \
         u.totp_secret_encrypted, u.totp_nonce, u.totp_enabled, u.totp_recovery_hashes, u.is_bot, u.locale, u.theme_mode, u.theme_palette, u.theme_scale, u.home_landing, u.density, \
         u.pronouns, u.profile_links, u.timezone \
         FROM sessions s \
         JOIN users u ON u.id = s.user_id \
         WHERE s.id = ? AND s.expires_at > datetime('now')",
    )
    .bind(hash_session_token(session_id))
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_user_record))
}

/// LC-100: set (or clear, with `None`) a user's preferred UI locale.
pub async fn set_user_locale(
    pool: &SqlitePool,
    user_id: &str,
    locale: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET locale = ? WHERE id = ?")
        .bind(locale)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-194: set (or clear, with `None`) a user's preferred UI density.
pub async fn set_user_density(
    pool: &SqlitePool,
    user_id: &str,
    density: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET density = ? WHERE id = ?")
        .bind(density)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-541: set (or clear, with `None`) a user's preferred UI mode.
pub async fn set_user_theme_mode(
    pool: &SqlitePool,
    user_id: &str,
    mode: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET theme_mode = ? WHERE id = ?")
        .bind(mode)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-541: set (or clear, with `None`) a user's preferred palette.
pub async fn set_user_theme_palette(
    pool: &SqlitePool,
    user_id: &str,
    palette: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET theme_palette = ? WHERE id = ?")
        .bind(palette)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-569: set (or clear, with `None`) a user's preferred UI scale.
pub async fn set_user_theme_scale(
    pool: &SqlitePool,
    user_id: &str,
    scale: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET theme_scale = ? WHERE id = ?")
        .bind(scale)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-575: set (or clear, with `None`) a user's "Open on" landing preference.
pub async fn set_user_home_landing(
    pool: &SqlitePool,
    user_id: &str,
    landing: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET home_landing = ? WHERE id = ?")
        .bind(landing)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(lookup_session_id(session_id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_user_sessions(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_users(pool: &SqlitePool) -> Result<Vec<UserRecord>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at, read_receipts_enabled, \
         bio, avatar_ext, status, custom_status, last_active_at, is_profile_public, \
         notify_browser_enabled, notify_sound_enabled, notify_push_enabled, \
         notify_email_digest_enabled, notify_login_alerts_enabled, \
         notify_email_activity_enabled, \
         last_ws_seen_at, last_digest_sent_at, \
         dnd_schedule_json, dnd_paused_until, email, \
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes, is_bot, locale, theme_mode, theme_palette, theme_scale, home_landing, density, \
         pronouns, profile_links, timezone \
         FROM users ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_user_record).collect())
}

pub async fn delete_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_invite_code(
    pool: &SqlitePool,
    code: &str,
    created_by: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO invite_codes (code, created_by) VALUES (?, ?)")
        .bind(code)
        .bind(created_by)
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}

pub async fn list_invite_codes(pool: &SqlitePool) -> Result<Vec<InviteCode>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, code, created_by, used_by, used_at, expires_at, created_at \
         FROM invite_codes ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| InviteCode {
            id: r.get("id"),
            code: r.get("code"),
            created_by: r.get("created_by"),
            used_by: r.get("used_by"),
            used_at: r.get("used_at"),
            expires_at: r.get("expires_at"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn revoke_invite_code(pool: &SqlitePool, code_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM invite_codes WHERE id = ?")
        .bind(code_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn redeem_invite_code(
    pool: &SqlitePool,
    code: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE invite_codes SET used_by = ?, used_at = datetime('now') \
         WHERE code = ? AND used_by IS NULL",
    )
    .bind(user_id)
    .bind(code)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_invite_code(
    pool: &SqlitePool,
    code: &str,
) -> Result<Option<InviteCode>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, code, created_by, used_by, used_at, expires_at, created_at \
         FROM invite_codes WHERE code = ?",
    )
    .bind(code)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| InviteCode {
        id: r.get("id"),
        code: r.get("code"),
        created_by: r.get("created_by"),
        used_by: r.get("used_by"),
        used_at: r.get("used_at"),
        expires_at: r.get("expires_at"),
        created_at: r.get("created_at"),
    }))
}

pub async fn ban_user(
    pool: &SqlitePool,
    user_id: &str,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 1, ban_reason = ?, banned_until = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unban_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 0, ban_reason = NULL, banned_until = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn suspend_user(
    pool: &SqlitePool,
    user_id: &str,
    until: &str,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_banned = 1, ban_reason = ?, banned_until = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(reason)
    .bind(until)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mute_user(
    pool: &SqlitePool,
    user_id: &str,
    until: Option<&str>,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_muted = 1, muted_until = ?, mute_reason = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(until)
    .bind(reason)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unmute_user(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_muted = 0, muted_until = NULL, mute_reason = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-535: null out every timed mute whose `muted_until` has passed, so the
/// admin table and exports stop reporting an expired mute. A NULL
/// `muted_until` is a permanent mute and is never touched. Both the stored
/// value and `datetime('now')` are UTC `YYYY-MM-DD HH:MM:SS` strings, so the
/// lexical `<` comparison is chronological. Returns the number of rows
/// cleared. Posting gates already honour expiry via `User::mute_in_effect`,
/// so this sweep is purely for DB truthfulness, not enforcement.
pub async fn clear_expired_mutes(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE users SET is_muted = 0, muted_until = NULL, mute_reason = NULL, \
         updated_at = datetime('now') \
         WHERE is_muted = 1 AND muted_until IS NOT NULL AND muted_until < datetime('now')",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

#[allow(clippy::too_many_arguments)]
pub async fn update_user_profile(
    pool: &SqlitePool,
    user_id: &str,
    display_name: Option<&str>,
    bio: Option<&str>,
    // LC-533: profile extras, already validated + normalised by the caller.
    pronouns: Option<&str>,
    profile_links: Option<&str>,
    timezone: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET display_name = ?, bio = ?, pronouns = ?, profile_links = ?, \
         timezone = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(display_name)
    .bind(bio)
    .bind(pronouns)
    .bind(profile_links)
    .bind(timezone)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_user_avatar_ext(
    pool: &SqlitePool,
    user_id: &str,
    ext: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET avatar_ext = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(ext)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_read_receipts_enabled(
    pool: &SqlitePool,
    user_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET read_receipts_enabled = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(enabled as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_notification_prefs(
    pool: &SqlitePool,
    user_id: &str,
    browser: bool,
    sound: bool,
    push: bool,
    email_digest: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users \
            SET notify_browser_enabled       = ?, \
                notify_sound_enabled         = ?, \
                notify_push_enabled          = ?, \
                notify_email_digest_enabled  = ?, \
                updated_at                   = datetime('now') \
          WHERE id = ?",
    )
    .bind(browser as i32)
    .bind(sound as i32)
    .bind(push as i32)
    .bind(email_digest as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_notify_login_alerts_enabled(
    pool: &SqlitePool,
    user_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET notify_login_alerts_enabled = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(enabled as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-580: the ISO 3166-1 alpha-2 country the user last logged in from, or
/// `None` until the first geolocatable login. Compared at the SSO callback to
/// detect a significant location change.
pub async fn get_last_login_country(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT last_login_country FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.flatten())
}

/// LC-580: record the ISO 3166-1 alpha-2 country of the user's most recent
/// geolocatable login, for the next login's change comparison.
pub async fn set_last_login_country(
    pool: &SqlitePool,
    user_id: &str,
    country: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET last_login_country = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(country)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-88: persist a user's DND schedule. `schedule_json` is `None` to clear
/// the schedule (no quiet hours). The caller is responsible for validating
/// the JSON shape before storing.
pub async fn set_dnd_schedule(
    pool: &SqlitePool,
    user_id: &str,
    schedule_json: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET dnd_schedule_json = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(schedule_json)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-88: set or clear the manual pause instant. `paused_until` is an
/// ISO-8601 UTC string, or `None` to resume immediately.
pub async fn set_dnd_pause(
    pool: &SqlitePool,
    user_id: &str,
    paused_until: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET dnd_paused_until = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(paused_until)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Toggle the digest-opt-in flag for `user_id`. Used by the register
/// flow when the operator has flipped `default_notify_email_digest` to
/// `1`: new users start opted in, but the column default in the schema
/// is `0` so this helper performs the targeted update.
pub async fn set_notify_email_digest_enabled(
    pool: &SqlitePool,
    user_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET notify_email_digest_enabled = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(enabled as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-77-REPLY: toggle the per-message mention + DM email opt-in. Separate
/// from `set_notification_prefs` so the existing 4-arg bulk-update signature
/// stays unchanged for callers that only care about the original notification
/// toggles.
pub async fn set_notify_email_activity_enabled(
    pool: &SqlitePool,
    user_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET notify_email_activity_enabled = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(enabled as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-526 follow-up: whether the user has opted out of the public kudos
/// leaderboard. Queried on demand (settings render); not carried on the User
/// projection since it is needed in only two places.
pub async fn get_kudos_opt_out(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT kudos_leaderboard_opt_out FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(v.unwrap_or(0) != 0)
}

/// LC-526 follow-up: set the kudos-leaderboard opt-out flag.
pub async fn set_kudos_opt_out(
    pool: &SqlitePool,
    user_id: &str,
    opt_out: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET kudos_leaderboard_opt_out = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(opt_out as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-526 follow-up: ids of all users who opted out of the kudos leaderboard,
/// for excluding them from the aggregate (the leaderboard lives in a different
/// db, so the exclusion is applied as a NOT IN list rather than a join).
pub async fn kudos_opted_out_ids(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows =
        sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE kudos_leaderboard_opt_out = 1")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

/// Bulk-resolve display fields for a set of user ids in a single query.
/// Returns a map keyed by user id with `(username, display_name)`.
/// Missing ids are absent from the map; callers fall back to the raw id
/// LC-489: the subset of `ids` whose owner has `read_receipts_enabled = 1`.
/// Used to filter a room's caught-up members down to those who consented to
/// broadcasting their read status before showing them in a "Seen by" stack.
pub async fn read_receipts_enabled_ids(
    pool: &SqlitePool,
    ids: &[&str],
) -> Result<HashSet<String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("SELECT id FROM users WHERE read_receipts_enabled = 1 AND id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>("id")).collect())
}

/// for unknown / deleted users. Used by callers that already have the
/// id list in hand (e.g. pinned-message render) to avoid N+1 lookups
/// across the chat/auth pool boundary.
pub async fn display_names_for_ids(
    pool: &SqlitePool,
    ids: &[&str],
) -> Result<std::collections::HashMap<String, (String, Option<String>)>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT id, username, display_name FROM users WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let id: String = r.get("id");
        let username: String = r.get("username");
        let display_name: Option<String> = r.get("display_name");
        map.insert(id, (username, display_name));
    }
    Ok(map)
}

/// Bulk-load id + username + status for a set of user ids. Used by
/// `@here` resolution to filter out DND users in one query rather than N.
/// Banned users are excluded (same as `list_user_ids`); their messages and
/// mentions are hidden everywhere else, so they should not receive a
/// broadcast ping either.
pub async fn usernames_and_status_for_ids(
    pool: &SqlitePool,
    ids: &[&str],
) -> Result<std::collections::HashMap<String, (String, String)>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, username, status FROM users \
         WHERE id IN ({placeholders}) AND is_banned = 0"
    );
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let id: String = r.get("id");
        let username: String = r.get("username");
        let status: String = r.get("status");
        map.insert(id, (username, status));
    }
    Ok(map)
}

/// All user IDs in the auth DB. Used by the autocomplete fallback when a
/// public room has no enclave assigned.
pub async fn list_user_ids(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT id FROM users WHERE is_banned = 0 ORDER BY username COLLATE NOCASE")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>("id")).collect())
}

/// Case-insensitive substring match on username and display_name. Excludes
/// banned users and private profiles (except the viewer themselves). Results
/// sorted by username, capped at `limit`. SQL LIKE wildcards (`%`, `_`)
/// inside the input are escaped so callers cannot use them to broaden the
/// match.
pub async fn search_users(
    pool: &SqlitePool,
    q: &str,
    viewer_id: &str,
    limit: i64,
) -> Result<Vec<UserRecord>, sqlx::Error> {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let rows = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at, read_receipts_enabled, \
         bio, avatar_ext, status, custom_status, last_active_at, is_profile_public, \
         notify_browser_enabled, notify_sound_enabled, notify_push_enabled, \
         notify_email_digest_enabled, notify_login_alerts_enabled, \
         notify_email_activity_enabled, \
         last_ws_seen_at, last_digest_sent_at, \
         dnd_schedule_json, dnd_paused_until, email, \
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes, is_bot, locale, theme_mode, theme_palette, theme_scale, home_landing, density, \
         pronouns, profile_links, timezone \
         FROM users \
         WHERE is_banned = 0 \
           AND (is_profile_public = 1 OR id = ?) \
           AND id NOT IN ( \
             SELECT blocked_id FROM user_blocks WHERE blocker_id = ? \
             UNION \
             SELECT blocker_id FROM user_blocks WHERE blocked_id = ? \
           ) \
           AND (username LIKE ? ESCAPE '\\' COLLATE NOCASE \
             OR display_name LIKE ? ESCAPE '\\' COLLATE NOCASE) \
         ORDER BY username COLLATE NOCASE \
         LIMIT ?",
    )
    .bind(viewer_id)
    .bind(viewer_id)
    .bind(viewer_id)
    .bind(&pattern)
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_user_record).collect())
}

pub async fn set_profile_public(
    pool: &SqlitePool,
    user_id: &str,
    is_public: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET is_profile_public = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(is_public as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a block edge. No-op if it already exists. The CHECK constraint on
/// the table prevents self-blocks at the SQL layer; route handlers should
/// also validate so they can return a friendlier error than a constraint
/// violation.
pub async fn block_user(
    pool: &SqlitePool,
    blocker_id: &str,
    blocked_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO user_blocks (blocker_id, blocked_id) VALUES (?, ?)")
        .bind(blocker_id)
        .bind(blocked_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn unblock_user(
    pool: &SqlitePool,
    blocker_id: &str,
    blocked_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_blocks WHERE blocker_id = ? AND blocked_id = ?")
        .bind(blocker_id)
        .bind(blocked_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// True when `viewer_id` has blocked `other_id` (one direction only). Used
/// to drive the Block/Unblock button label.
pub async fn did_block(
    pool: &SqlitePool,
    viewer_id: &str,
    other_id: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM user_blocks WHERE blocker_id = ? AND blocked_id = ?")
        .bind(viewer_id)
        .bind(other_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// True when either user has blocked the other. The blocking effect is
/// symmetric for visibility and messaging: the blocker should not see the
/// blockee, and the blockee should not be able to see or contact the
/// blocker.
pub async fn is_blocked_either_way(
    pool: &SqlitePool,
    a: &str,
    b: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT 1 FROM user_blocks \
         WHERE (blocker_id = ? AND blocked_id = ?) \
            OR (blocker_id = ? AND blocked_id = ?)",
    )
    .bind(a)
    .bind(b)
    .bind(b)
    .bind(a)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// All user IDs that the viewer has blocked OR that have blocked the viewer.
/// Used by message renderers to hide content authored by anyone in this set.
pub async fn list_blocked_ids_either_way(
    pool: &SqlitePool,
    viewer_id: &str,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT blocked_id AS other_id FROM user_blocks WHERE blocker_id = ? \
         UNION \
         SELECT blocker_id AS other_id FROM user_blocks WHERE blocked_id = ?",
    )
    .bind(viewer_id)
    .bind(viewer_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("other_id"))
        .collect())
}

/// LC-698: the row that owns `email` (case-insensitive), with the flags the SSO
/// resolver needs to classify it. `users.email` is uniquely indexed, so at most
/// one row can match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailOwner {
    pub id: String,
    /// The subject this row is already linked to, or `None` when unlinked
    /// (NULL or the empty string, both of which LC-588 treated as unlinked).
    pub bunyip_sub: Option<String>,
    pub is_bot: bool,
    pub is_banned: bool,
    /// `email_verified_at IS NOT NULL`: the stored address was verified by an
    /// authority. A self-service profile email is never verified (LC-22 retired
    /// verification mail and `set_user_email` clears the stamp on change).
    pub email_verified: bool,
}

/// LC-698: the subject a row is linked to, or `None` when it is unlinked (the
/// empty-string marker) or the user is gone. Drives the admin row's "Unlink SSO"
/// affordance, which is only offered when there is a subject to clear.
pub async fn get_bunyip_sub(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let sub: Option<String> = sqlx::query_scalar("SELECT bunyip_sub FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(sub.filter(|s| !s.trim().is_empty()))
}

/// LC-698: look up the row owning `email`, whatever its state. Replaces
/// `find_user_id_by_email`, which filtered banned rows and returned only an id:
/// the SSO resolver must see `bunyip_sub`, `is_bot` and the verified stamp to
/// tell an adoptable row from one it has to refuse.
pub async fn find_email_owner(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<EmailOwner>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, bunyip_sub, COALESCE(is_bot, 0) AS is_bot, is_banned, email_verified_at \
         FROM users WHERE email = ? COLLATE NOCASE",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| EmailOwner {
        id: r.get::<String, _>("id"),
        bunyip_sub: r
            .get::<Option<String>, _>("bunyip_sub")
            .filter(|s| !s.trim().is_empty()),
        is_bot: r.get::<i64, _>("is_bot") != 0,
        is_banned: r.get::<i64, _>("is_banned") != 0,
        email_verified: r
            .get::<Option<String>, _>("email_verified_at")
            .is_some_and(|s| !s.trim().is_empty()),
    }))
}

/// LC-588 (restored by LC-698): link an UNLINKED local account to a bunyip
/// subject, matched by verified email. Returns whether a row was linked.
///
/// The `bunyip_sub IS NULL OR ''` guard is the takeover guard: a row already
/// claimed by some other subject can never be re-pointed here, so an email
/// string (which any user can set on their own profile, unverified) cannot bind
/// an incoming identity onto someone else's account. LC-618 deleted the clause
/// to auto-relink a ROTATED sub; that silent relink is the vulnerability LC-698
/// closes. A rotated sub is now an explicit identity conflict, recoverable by an
/// admin through `clear_bunyip_sub`.
///
/// The `is_bot` guard is retained: bot rows are never linked (the caller turns a
/// `false` return into an identity conflict rather than provisioning a duplicate).
/// On success every session on the row is deleted, so a link can never leave a
/// previous holder's cookie live.
pub async fn link_bunyip_sub(
    pool: &SqlitePool,
    user_id: &str,
    sub: &str,
) -> Result<bool, sqlx::Error> {
    // The empty string is the UNLINKED marker, never a subject to link to.
    if sub.trim().is_empty() {
        return Ok(false);
    }
    let res = sqlx::query(
        "UPDATE users SET bunyip_sub = ?, updated_at = datetime('now') \
         WHERE id = ? AND COALESCE(is_bot, 0) = 0 \
           AND (bunyip_sub IS NULL OR bunyip_sub = '')",
    )
    .bind(sub)
    .bind(user_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    delete_user_sessions(pool, user_id).await?;
    Ok(true)
}

/// LC-698: unlink a user from its bunyip subject, so the next SSO login for
/// that verified email re-links the row through `link_bunyip_sub`. This is the
/// deliberate admin path out of an identity conflict (the OP rotated the sub).
/// Returns whether a row changed; bot rows and already-unlinked rows report
/// `false` so the caller can surface a no-op rather than claim success. Every
/// session on the row is deleted, so an authorized relink cannot leave the
/// previous holder signed in.
///
/// Unlinked is the empty string, not NULL: `bunyip_sub` is `NOT NULL DEFAULT ''`
/// (migration 0029), and 0043 made its unique index partial so any number of
/// rows may sit unlinked at once.
pub async fn clear_bunyip_sub(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE users SET bunyip_sub = '', updated_at = datetime('now') \
         WHERE id = ? AND COALESCE(is_bot, 0) = 0 \
           AND bunyip_sub IS NOT NULL AND bunyip_sub <> ''",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    delete_user_sessions(pool, user_id).await?;
    Ok(true)
}

/// Fetch the configured email for a user, or `None` if unset. Used to render
/// the email on the profile settings page and to address outbound mail.
pub async fn get_user_email(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT email FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("email")))
}

/// Update (or clear with `None`) a user's email. Returns `Err` with a unique
/// violation if another row already owns this address; the caller maps that
/// to a friendly form error.
///
/// If the address actually changes (case-sensitive compare against the
/// stored value), `email_verified_at` is cleared in the same statement so a
/// previously verified address never silently transfers its verified state
/// to a new one. Re-saving the same address is a no-op.
pub async fn set_user_email(
    pool: &SqlitePool,
    user_id: &str,
    email: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET \
            email = ?1, \
            email_verified_at = CASE \
                WHEN COALESCE(email, '') = COALESCE(?1, '') THEN email_verified_at \
                ELSE NULL \
            END, \
            updated_at = datetime('now') \
         WHERE id = ?2",
    )
    .bind(email)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch the verified-at timestamp for a user's email, or `None` if the
/// current email is unverified (or no email is set).
pub async fn get_user_email_verified_at(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT email_verified_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("email_verified_at")))
}

/// Stamp the user's email as verified, but only when their currently-stored
/// address still matches `email`. The guard prevents a token issued before
/// an in-flight email change from verifying the new address. Returns the
/// number of rows updated so the caller can detect a no-op (token stale
/// relative to the current email).
pub async fn mark_email_verified(
    pool: &SqlitePool,
    user_id: &str,
    email: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE users SET email_verified_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ? AND email = ?",
    )
    .bind(user_id)
    .bind(email)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// LC-627: stamp `email_verified_at` when it is not already set. Used by the SSO
/// callback: the identity provider (bunyip) is the authority for whether an
/// address is verified. `IS NULL` keeps it idempotent - the first verified SSO
/// login sets it, later logins are no-ops (no `updated_at` churn).
///
/// LC-698 adds the `email` guard: the stamp asserts that THIS address was
/// verified, so it may only be written when the stored address is the one the OP
/// vouched for. Without it a user could set an arbitrary unverified profile
/// email, log in, and have their own login stamp that stranger's address as
/// verified - which would defeat the resolver's verified-email adoption gate.
pub async fn mark_email_verified_if_unset(
    pool: &SqlitePool,
    user_id: &str,
    email: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE users SET email_verified_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ? AND email_verified_at IS NULL AND email = ? COLLATE NOCASE",
    )
    .bind(user_id)
    .bind(email)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Overwrite a user's password hash. Used by the reset flow after a token
/// has been validated. Callers should also delete every session for this
/// user so any existing logged-in browser is force-signed-out.
pub async fn set_password_hash(
    pool: &SqlitePool,
    user_id: &str,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(password_hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// All users `blocker_id` has blocked, ordered by username.
pub async fn list_blocked_users(
    pool: &SqlitePool,
    blocker_id: &str,
) -> Result<Vec<UserRecord>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT u.id, u.username, u.display_name, u.password_hash, u.role, \
         u.is_banned, u.ban_reason, u.banned_until, \
         u.is_muted, u.muted_until, u.mute_reason, \
         u.created_at, u.updated_at, u.read_receipts_enabled, \
         u.bio, u.avatar_ext, u.status, u.custom_status, u.last_active_at, u.is_profile_public, \
         u.notify_browser_enabled, u.notify_sound_enabled, u.notify_push_enabled, \
         u.notify_email_digest_enabled, u.notify_login_alerts_enabled, \
         u.notify_email_activity_enabled, \
         u.last_ws_seen_at, u.last_digest_sent_at, \
         u.dnd_schedule_json, u.dnd_paused_until, u.email, \
         u.totp_secret_encrypted, u.totp_nonce, u.totp_enabled, u.totp_recovery_hashes, u.is_bot, u.locale, u.theme_mode, u.theme_palette, u.theme_scale, u.home_landing, u.density, \
         u.pronouns, u.profile_links, u.timezone \
         FROM user_blocks b \
         JOIN users u ON u.id = b.blocked_id \
         WHERE b.blocker_id = ? \
         ORDER BY u.username COLLATE NOCASE",
    )
    .bind(blocker_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_user_record).collect())
}

// =====================================================================
// LC-22: Bunyip SSO resolver helpers (pure-RP cutover).
// =====================================================================

/// Returns the lets-chat `users.id` whose `bunyip_sub` column matches the
/// supplied verified `sub` claim. `None` on no match.
///
/// LC-698: `bunyip_sub <> ''` because the empty string is the UNLINKED marker,
/// not a subject. Without it an empty `sub` would resolve to an arbitrary
/// unlinked account.
pub async fn find_user_id_by_bunyip_sub(
    pool: &SqlitePool,
    sub: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM users WHERE bunyip_sub = ? AND bunyip_sub <> ''",
    )
    .bind(sub)
    .fetch_optional(pool)
    .await
}

/// LC-22: lookup by id with the moderation flags the callback resolver
/// needs. Returns banned + bot booleans alongside the id, so the callback
/// can short-circuit a banned account or refuse to attach a sub to a bot.
/// LC-698: the empty string never matches (see `find_user_id_by_bunyip_sub`).
pub async fn get_user_auth_flags_by_bunyip_sub(
    pool: &SqlitePool,
    sub: &str,
) -> Result<Option<(String, bool, bool)>, sqlx::Error> {
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT id, is_banned, COALESCE(is_bot, 0) FROM users \
         WHERE bunyip_sub = ? AND bunyip_sub <> ''",
    )
    .bind(sub)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, banned, bot)| (id, banned != 0, bot != 0)))
}

/// LC-413: read the current `users.role` for an SSO-resolved user so the
/// callback can skip a no-op UPDATE (and avoid a log line) when the Bunyip
/// admin claim already matches the local row.
pub async fn get_user_role(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

/// LC-414: read the `bunyip_sub` stamped on a user row so the per-request
/// identity-swap check can compare it against the `sub` claim of any
/// Bunyip `access_token` cookie that rides the request. A `NULL` return
/// is a leftover pre-cutover row; post-LC-22 the column is `NOT NULL`,
/// so the swap check just no-ops on that row.
pub async fn get_user_bunyip_sub(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT bunyip_sub FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(|opt: Option<Option<String>>| opt.flatten())
}

/// LC-22: create a fresh lets-chat user row from a verified Bunyip identity.
///
/// `password_hash` is written as the empty string (the cutover sentinel - see
/// migration 0032). `bunyip_sub` is NOT NULL UNIQUE post-cutover.
///
/// First-user-to-admin promotion is handled outside this helper because the
/// count-then-promote sequence needs the parent transaction; see
/// `routes::bunyip_sso::resolve_or_provision_user`.
pub async fn create_user_from_bunyip(
    pool: &SqlitePool,
    username: &str,
    bunyip_sub: &str,
    display_name: Option<&str>,
    email: Option<&str>,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, bunyip_sub, display_name, email, last_active_at) \
         VALUES (?, ?, '', ?, ?, ?, datetime('now'))",
    )
    .bind(&id)
    .bind(username)
    .bind(bunyip_sub)
    .bind(display_name)
    .bind(email)
    .execute(pool)
    .await?;
    Ok(id)
}

/// LC-22: returns true when the username is already taken (case-insensitive
/// per the `users.username COLLATE NOCASE` unique constraint).
pub async fn username_exists(pool: &SqlitePool, username: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = ? COLLATE NOCASE")
        .bind(username)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

// ── LC-587: suspicious-login approval + known-device tracking ──────────────

/// A pending suspicious-login approval challenge, read back for verification.
#[derive(Debug, Clone)]
pub struct LoginApproval {
    pub user_id: String,
    pub code_hash: String,
    pub country: Option<String>,
    pub device_hash: Option<String>,
    pub attempts: i64,
}

/// LC-587: insert a pending approval. `token` is the opaque single-use lookup
/// key (also handed to the browser); `code_hash` is SHA-256(6-digit code). The
/// row expires in 15 minutes and is single-use (`consumed_at`).
#[allow(clippy::too_many_arguments)]
pub async fn insert_login_approval(
    pool: &SqlitePool,
    token: &str,
    user_id: &str,
    code_hash: &str,
    country: Option<&str>,
    device_hash: Option<&str>,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO login_approvals \
         (id, user_id, code_hash, country, device_hash, ip, user_agent, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now', '+15 minutes'))",
    )
    .bind(token)
    .bind(user_id)
    .bind(code_hash)
    .bind(country)
    .bind(device_hash)
    .bind(ip)
    .bind(user_agent)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-587: atomically claim one guess against a challenge and return the row.
///
/// The single statement is both the validity check and the attempt counter, so
/// the number of codes that ever get compared is bounded by `max_attempts` no
/// matter how many submissions arrive at once. The previous read-then-compare
/// -then-bump sequence did not bound anything: concurrent submissions all read
/// `attempts` before any of them incremented it, so a burst of 40 requests got
/// 40 comparisons against a cap of 5. Same read-then-act shape LC-601 closed on
/// the consume path.
///
/// Returns `None` when the token is unknown, expired, already consumed, or has
/// no attempts left. The returned `attempts` is this caller's slot number, so
/// `attempts >= max_attempts` means the caller took the last one.
///
/// A correct code also spends a slot. That is harmless: success consumes the
/// challenge outright, so the slot cannot be reused either way.
pub async fn claim_login_approval_attempt(
    pool: &SqlitePool,
    token: &str,
    max_attempts: i64,
) -> Result<Option<LoginApproval>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE login_approvals \
            SET attempts = attempts + 1 \
          WHERE id = ? \
            AND consumed_at IS NULL \
            AND expires_at > datetime('now') \
            AND attempts < ? \
      RETURNING user_id, code_hash, country, device_hash, attempts",
    )
    .bind(token)
    .bind(max_attempts)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| LoginApproval {
        user_id: r.get::<String, _>("user_id"),
        code_hash: r.get::<String, _>("code_hash"),
        country: r.get::<Option<String>, _>("country"),
        device_hash: r.get::<Option<String>, _>("device_hash"),
        attempts: r.get::<i64, _>("attempts"),
    }))
}

/// LC-587: mark a challenge consumed so it can never be replayed (on success,
/// or when the attempt cap is hit).
/// Returns the number of rows consumed: 1 for the caller that won, 0 if the
/// challenge was already consumed. The `consumed_at IS NULL` guard makes this
/// the atomic single-use gate - `verify` mints a session only when it consumed
/// the row itself, so two concurrent correct-code submits cannot both succeed
/// (LC-601).
pub async fn consume_login_approval(pool: &SqlitePool, token: &str) -> Result<u64, sqlx::Error> {
    let res =
        sqlx::query("UPDATE login_approvals SET consumed_at = datetime('now') WHERE id = ? AND consumed_at IS NULL")
            .bind(token)
            .execute(pool)
            .await?;
    Ok(res.rows_affected())
}

/// LC-587: does the user have at least one known login device? Used to keep the
/// first device (or first-ever login) a silent baseline rather than a
/// new-device signal.
pub async fn has_known_device(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_login_devices WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// LC-587: is this exact device hash already known for the user?
pub async fn is_known_device(
    pool: &SqlitePool,
    user_id: &str,
    device_hash: &str,
) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_login_devices WHERE user_id = ? AND device_hash = ?",
    )
    .bind(user_id)
    .bind(device_hash)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

/// LC-587: record (or refresh) a known device for the user. Idempotent on the
/// `(user_id, device_hash)` unique key: a repeat login refreshes `last_seen_at`
/// and the user agent.
pub async fn record_known_device(
    pool: &SqlitePool,
    user_id: &str,
    device_hash: &str,
    user_agent: Option<&str>,
) -> Result<(), sqlx::Error> {
    use rand::Rng;
    let id: String = rand::rngs::OsRng
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    sqlx::query(
        "INSERT INTO user_login_devices (id, user_id, device_hash, user_agent) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, device_hash) \
         DO UPDATE SET last_seen_at = datetime('now'), user_agent = excluded.user_agent",
    )
    .bind(id)
    .bind(user_id)
    .bind(device_hash)
    .bind(user_agent)
    .execute(pool)
    .await?;
    Ok(())
}
