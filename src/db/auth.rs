use sqlx::{Row, SqlitePool};

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
