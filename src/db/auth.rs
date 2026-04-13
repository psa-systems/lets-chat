use sqlx::{Row, SqlitePool};

use crate::models::invite::InviteCode;
use crate::models::user::UserRecord;

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, ?)")
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
         created_at, updated_at \
         FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserRecord {
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
    }))
}

pub async fn find_user_by_id(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, username, display_name, password_hash, role, \
         is_banned, ban_reason, banned_until, \
         is_muted, muted_until, mute_reason, \
         created_at, updated_at \
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserRecord {
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
    }))
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

pub async fn create_session(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<String, sqlx::Error> {
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at) \
         VALUES (?, ?, datetime('now', '+30 days'))",
    )
    .bind(&token)
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(token)
}

pub async fn get_user_by_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT u.id, u.username, u.display_name, u.password_hash, u.role, \
         u.is_banned, u.ban_reason, u.banned_until, \
         u.is_muted, u.muted_until, u.mute_reason, \
         u.created_at, u.updated_at \
         FROM sessions s \
         JOIN users u ON u.id = s.user_id \
         WHERE s.id = ? AND s.expires_at > datetime('now')",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserRecord {
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
    }))
}

pub async fn delete_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_user_sessions(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<(), sqlx::Error> {
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
         created_at, updated_at \
         FROM users ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UserRecord {
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
        })
        .collect())
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
    let result = sqlx::query(
        "INSERT INTO invite_codes (code, created_by) VALUES (?, ?)",
    )
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
