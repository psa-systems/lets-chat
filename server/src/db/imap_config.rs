//! LC-77: IMAP-poll configuration stored at rest with the password
//! AES-256-GCM-sealed under the process secret key. Mirrors the
//! `crate::db::vapid` persistence pattern: singleton row at `id = 1`,
//! sealed BLOB + nonce columns, plaintext config fields alongside.
//!
//! The IMAP poll loop reads this row at startup and refuses to spawn if
//! any of {host, username, password, ingress_domain} is unset or if
//! `enabled = 0`. Flipping `enabled` to 1 (or setting fields on a fresh
//! row) requires a server restart to take effect; the spawn function
//! checks at startup, not per tick, matching the
//! `LETS_CHAT_RETENTION_SWEEP_ENABLED` + retention sweeper shape.

use sqlx::{Row, SqlitePool};

use crate::crypto;

/// Decrypted, in-memory IMAP poll configuration. Constructed from a row of
/// `imap_inbox_config` plus the process secret key. The `password` field is
/// the plaintext credential; it lives in memory only for the lifetime of
/// the poll-loop task that owns it.
#[derive(Debug, Clone)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password: String,
    pub folder: String,
    pub ingress_domain: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ImapConfigError {
    #[error("crypto: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("sql: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("decoded password is not valid utf-8")]
    PasswordEncoding,
    #[error("port {0} does not fit in u16")]
    PortRange(i64),
}

/// Read the singleton row and decrypt the password. Returns `None` if no
/// row exists. The caller is the IMAP poll loop spawn gate; it treats
/// `None` (or a row with `enabled = 0`) as "do not spawn".
pub async fn read(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<Option<ImapConfig>, ImapConfigError> {
    let row = sqlx::query(
        "SELECT host, port, tls, username, password_encrypted, password_nonce, \
                folder, ingress_domain, enabled \
         FROM imap_inbox_config WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let port_i64: i64 = r.get("port");
    let port: u16 = u16::try_from(port_i64).map_err(|_| ImapConfigError::PortRange(port_i64))?;
    let encrypted: Vec<u8> = r.get("password_encrypted");
    let nonce: Vec<u8> = r.get("password_nonce");
    let password_bytes = crypto::open(secret_key, &nonce, &encrypted)?;
    let password =
        String::from_utf8(password_bytes).map_err(|_| ImapConfigError::PasswordEncoding)?;
    Ok(Some(ImapConfig {
        host: r.get("host"),
        port,
        tls: r.get::<i64, _>("tls") != 0,
        username: r.get("username"),
        password,
        folder: r.get("folder"),
        ingress_domain: r.get("ingress_domain"),
        enabled: r.get::<i64, _>("enabled") != 0,
    }))
}

/// Encrypt the password and upsert the singleton row. Called from the
/// admin settings handler in commit 4. Idempotent: an existing row is
/// overwritten (including the nonce, since the AES-GCM nonce must change
/// every time the ciphertext does).
pub async fn write(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    cfg: &ImapConfig,
) -> Result<(), ImapConfigError> {
    let (encrypted, nonce) = crypto::seal(secret_key, cfg.password.as_bytes())?;
    sqlx::query(
        "INSERT INTO imap_inbox_config \
             (id, host, port, tls, username, password_encrypted, password_nonce, \
              folder, ingress_domain, enabled, updated_at) \
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
             host                = excluded.host, \
             port                = excluded.port, \
             tls                 = excluded.tls, \
             username            = excluded.username, \
             password_encrypted  = excluded.password_encrypted, \
             password_nonce      = excluded.password_nonce, \
             folder              = excluded.folder, \
             ingress_domain      = excluded.ingress_domain, \
             enabled             = excluded.enabled, \
             updated_at          = excluded.updated_at",
    )
    .bind(&cfg.host)
    .bind(cfg.port as i64)
    .bind(cfg.tls as i64)
    .bind(&cfg.username)
    .bind(&encrypted)
    .bind(&nonce)
    .bind(&cfg.folder)
    .bind(cfg.ingress_domain.as_deref())
    .bind(cfg.enabled as i64)
    .execute(pool)
    .await?;
    Ok(())
}
