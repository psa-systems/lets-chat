use sqlx::{Row, SqlitePool};

pub async fn set_totp_secret(
    pool: &SqlitePool,
    user_id: &str,
    encrypted_secret: &[u8],
    nonce: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET totp_secret_encrypted = ?, totp_nonce = ?, \
         totp_enabled = 0, totp_recovery_hashes = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(encrypted_secret)
    .bind(nonce)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn enable_totp(
    pool: &SqlitePool,
    user_id: &str,
    recovery_hashes_json: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET totp_enabled = 1, totp_recovery_hashes = ?, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(recovery_hashes_json)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn disable_totp(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET totp_enabled = 0, totp_secret_encrypted = NULL, \
         totp_nonce = NULL, totp_recovery_hashes = NULL, \
         updated_at = datetime('now') WHERE id = ?",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_recovery_hashes(
    pool: &SqlitePool,
    user_id: &str,
    recovery_hashes_json: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET totp_recovery_hashes = ?, updated_at = datetime('now') \
         WHERE id = ?",
    )
    .bind(recovery_hashes_json)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_pending_2fa(pool: &SqlitePool, user_id: &str) -> Result<String, sqlx::Error> {
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    sqlx::query(
        "INSERT INTO pending_2fa (token, user_id, expires_at) \
         VALUES (?, ?, datetime('now', '+5 minutes'))",
    )
    .bind(&token)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn get_pending_2fa_user(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT user_id FROM pending_2fa \
         WHERE token = ? AND expires_at > datetime('now')",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<String, _>("user_id")))
}

pub async fn delete_pending_2fa(pool: &SqlitePool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pending_2fa WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_expired_pending_2fa(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pending_2fa WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?;
    Ok(())
}

/// A registration that has passed input validation but not yet completed 2FA
/// enrollment. The user row in `users` is only created once the TOTP code is
/// verified; until then the credentials and freshly-generated TOTP secret
/// live here so an abandoned registration leaves no account behind.
pub struct PendingRegistration {
    pub username: String,
    pub email: Option<String>,
    pub password_hash: String,
    pub totp_secret_encrypted: Vec<u8>,
    pub totp_nonce: Vec<u8>,
}

pub async fn create_pending_registration(
    pool: &SqlitePool,
    username: &str,
    email: Option<&str>,
    password_hash: &str,
    totp_secret_encrypted: &[u8],
    totp_nonce: &[u8],
) -> Result<String, sqlx::Error> {
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    sqlx::query(
        "INSERT INTO pending_registrations \
         (token, username, email, password_hash, totp_secret_encrypted, totp_nonce, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, datetime('now', '+30 minutes'))",
    )
    .bind(&token)
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(totp_secret_encrypted)
    .bind(totp_nonce)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn get_pending_registration(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<PendingRegistration>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT username, email, password_hash, totp_secret_encrypted, totp_nonce \
         FROM pending_registrations \
         WHERE token = ? AND expires_at > datetime('now')",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| PendingRegistration {
        username: r.get("username"),
        email: r.get("email"),
        password_hash: r.get("password_hash"),
        totp_secret_encrypted: r.get("totp_secret_encrypted"),
        totp_nonce: r.get("totp_nonce"),
    }))
}

pub async fn delete_pending_registration(
    pool: &SqlitePool,
    token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pending_registrations WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_expired_pending_registrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pending_registrations WHERE expires_at <= datetime('now')")
        .execute(pool)
        .await?;
    Ok(())
}
