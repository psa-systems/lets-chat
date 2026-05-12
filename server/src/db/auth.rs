use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

use crate::models::invite::InviteCode;
use crate::models::user::UserRecord;

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, last_active_at) \
         VALUES (?, ?, ?, datetime('now'))",
    )
    .bind(&id)
    .bind(username)
    .bind(password_hash)
    .execute(pool)
    .await?;
    Ok(id)
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
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes \
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
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes \
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
        totp_secret_encrypted: r.get("totp_secret_encrypted"),
        totp_nonce: r.get("totp_nonce"),
        totp_enabled: r.get("totp_enabled"),
        totp_recovery_hashes: r.get("totp_recovery_hashes"),
    }
}

/// Allowed values for the `users.status` column.
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_IDLE: &str = "idle";
pub const STATUS_DND: &str = "dnd";
pub const MAX_CUSTOM_STATUS_CHARS: usize = 50;

pub fn is_valid_status(s: &str) -> bool {
    matches!(s, STATUS_ACTIVE | STATUS_IDLE | STATUS_DND)
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

pub async fn set_user_status(
    pool: &SqlitePool,
    user_id: &str,
    status: &str,
    custom: Option<&str>,
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
    let sql = format!(
        "UPDATE users SET status = ?, custom_status = ?, updated_at = datetime('now'){now_clause} WHERE id = ?"
    );
    sqlx::query(&sql)
        .bind(status)
        .bind(custom)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Refresh `last_active_at`. If the user was `'idle'`, promote them back to
/// `'active'`. DND is sticky: bumps the timestamp so the idle clock restarts
/// once they leave DND, but never overwrites the status. Returns `true` only
/// when the call actually flipped idle->active so the caller can broadcast.
pub async fn touch_user_activity(pool: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT status FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(r) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let current: String = r.get("status");
    let flipped = current == STATUS_IDLE;
    if current == STATUS_DND {
        sqlx::query("UPDATE users SET last_active_at = datetime('now') WHERE id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(
            "UPDATE users SET last_active_at = datetime('now'), \
             status = 'active', updated_at = datetime('now') WHERE id = ?",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(flipped)
}

/// Flip rows whose `status = 'active'` and whose `last_active_at` is older
/// than `threshold_seconds` to `'idle'`. Returns the IDs that flipped.
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
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at, user_agent, ip, last_seen_at) \
         VALUES (?, ?, datetime('now', '+30 days'), ?, ?, datetime('now'))",
    )
    .bind(&token)
    .bind(user_id)
    .bind(user_agent)
    .bind(ip)
    .execute(pool)
    .await?;

    Ok(token)
}

/// Bump `last_seen_at` for a live session. Called from the auth middleware on
/// every authed request; throttled by `LAST_SEEN_REFRESH_SECONDS` so the write
/// rate stays well below the read rate.
pub async fn touch_session_last_seen(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET last_seen_at = datetime('now') WHERE id = ?")
        .bind(session_id)
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
pub async fn delete_session_for_user(
    pool: &SqlitePool,
    session_id: &str,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(session_id)
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
         u.totp_secret_encrypted, u.totp_nonce, u.totp_enabled, u.totp_recovery_hashes \
         FROM sessions s \
         JOIN users u ON u.id = s.user_id \
         WHERE s.id = ? AND s.expires_at > datetime('now')",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_user_record))
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
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
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes \
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

pub async fn update_user_profile(
    pool: &SqlitePool,
    user_id: &str,
    display_name: Option<&str>,
    bio: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET display_name = ?, bio = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(display_name)
    .bind(bio)
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
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users \
            SET notify_browser_enabled = ?, \
                notify_sound_enabled   = ?, \
                notify_push_enabled    = ?, \
                updated_at             = datetime('now') \
          WHERE id = ?",
    )
    .bind(browser as i32)
    .bind(sound as i32)
    .bind(push as i32)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Bulk-resolve display fields for a set of user ids in a single query.
/// Returns a map keyed by user id with `(username, display_name)`.
/// Missing ids are absent from the map; callers fall back to the raw id
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
         totp_secret_encrypted, totp_nonce, totp_enabled, totp_recovery_hashes \
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

/// Look up the user_id whose email matches `email` (case-insensitive). Used
/// by the password-reset request handler to map an inbound email address to
/// the account that should receive a reset link. Returns `None` for unknown
/// addresses; the caller still returns 200 to avoid email enumeration.
pub async fn find_user_id_by_email(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT id FROM users WHERE email = ? COLLATE NOCASE AND is_banned = 0")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<String, _>("id")))
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
         u.totp_secret_encrypted, u.totp_nonce, u.totp_enabled, u.totp_recovery_hashes \
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
