//! SMTP configuration storage (settings.db).
//!
//! Phase 22 task 2 introduced the typed `smtp_settings` table; this module is
//! its access layer. The password is encrypted at rest with AES-256-GCM under
//! a key derived from `LETS_CHAT_SECRET_KEY` (same crypto primitive as VAPID
//! and 2FA). If the secret key is missing, the password cannot be decrypted
//! and `load` returns `Err`; the admin page is expected to surface this and
//! the digest tick to short-circuit.

use sqlx::{Row, SqlitePool};

use crate::crypto;
use crate::error::AppError;

/// Decrypted, in-memory SMTP config. Constructed by `load`. `password` is
/// `None` when no password has ever been stored (the operator may be using
/// an open relay or has not yet configured one); the SMTP transport skips
/// auth when either `username` or `password` is absent.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub tls_mode: TlsMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Connect plain, upgrade with STARTTLS. Default for SMTP submission
    /// (port 587) and what every major provider expects.
    StartTls,
    /// Implicit TLS from the first byte (port 465 historically; some
    /// providers still prefer this).
    Tls,
    /// No TLS. Only safe for localhost relays.
    None,
}

impl TlsMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TlsMode::StartTls => "starttls",
            TlsMode::Tls => "tls",
            TlsMode::None => "none",
        }
    }

    pub fn parse(s: &str) -> TlsMode {
        match s {
            "tls" => TlsMode::Tls,
            "none" => TlsMode::None,
            // Unknown values fall back to starttls. This includes the
            // default the migration seeded and any future renames.
            _ => TlsMode::StartTls,
        }
    }
}

/// Input shape for `save`. `password: None` means "leave the existing stored
/// password alone." `password: Some(non_empty)` means "encrypt this and store."
/// `password: Some("")` is treated as "leave alone" so a blank form field does
/// not silently clear the saved password.
#[derive(Debug, Clone)]
pub struct SmtpConfigInput {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub tls_mode: TlsMode,
}

/// Load and decrypt the singleton SMTP config row.
///
/// Returns `Ok(None)` only when the row genuinely does not exist (which
/// should not happen post-migration, since 0004 inserts a default row).
/// Returns `Ok(Some(cfg))` with `cfg.host` possibly empty: callers gate on
/// `host.is_empty()` to decide whether SMTP is actually configured.
///
/// Decryption failure (wrong key, corrupted row) returns `Err`. That state
/// is operator-visible: the admin page renders a banner, the digest tick
/// logs and skips.
pub async fn load(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<Option<SmtpConfig>, AppError> {
    let row = sqlx::query(
        "SELECT host, port, username, password_encrypted, password_nonce, \
                from_address, tls_mode \
           FROM smtp_settings WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let password_encrypted: Option<Vec<u8>> = r.get("password_encrypted");
    let password_nonce: Option<Vec<u8>> = r.get("password_nonce");
    let password = match (password_encrypted, password_nonce) {
        (Some(ct), Some(nonce)) => {
            let plain = crypto::open(secret_key, &nonce, &ct)
                .map_err(|_| AppError::Internal("smtp password decrypt failed".into()))?;
            Some(
                String::from_utf8(plain)
                    .map_err(|_| AppError::Internal("smtp password is not utf-8".into()))?,
            )
        }
        _ => None,
    };
    let port: i64 = r.get("port");
    let tls_mode: String = r.get("tls_mode");
    Ok(Some(SmtpConfig {
        host: r.get("host"),
        port: port as u16,
        username: r.get("username"),
        password,
        from_address: r.get("from_address"),
        tls_mode: TlsMode::parse(&tls_mode),
    }))
}

/// Upsert the singleton row. `password: None` or `Some("")` preserves the
/// existing encrypted password; `password: Some(non_empty)` re-encrypts.
pub async fn save(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    input: &SmtpConfigInput,
) -> Result<(), AppError> {
    let new_password = match input.password.as_deref() {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    };
    if let Some(p) = new_password {
        let (ciphertext, nonce) = crypto::seal(secret_key, p.as_bytes())
            .map_err(|_| AppError::Internal("smtp password encrypt failed".into()))?;
        sqlx::query(
            "INSERT INTO smtp_settings \
                 (id, host, port, username, password_encrypted, password_nonce, \
                  from_address, tls_mode, updated_at) \
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET \
                 host = excluded.host, \
                 port = excluded.port, \
                 username = excluded.username, \
                 password_encrypted = excluded.password_encrypted, \
                 password_nonce = excluded.password_nonce, \
                 from_address = excluded.from_address, \
                 tls_mode = excluded.tls_mode, \
                 updated_at = datetime('now')",
        )
        .bind(&input.host)
        .bind(input.port as i64)
        .bind(input.username.as_deref())
        .bind(ciphertext)
        .bind(nonce)
        .bind(&input.from_address)
        .bind(input.tls_mode.as_str())
        .execute(pool)
        .await?;
    } else {
        // Leave password_encrypted/password_nonce alone. UPSERT with the
        // existing-row branch updating only the other columns.
        sqlx::query(
            "INSERT INTO smtp_settings \
                 (id, host, port, username, from_address, tls_mode, updated_at) \
             VALUES (1, ?, ?, ?, ?, ?, datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET \
                 host = excluded.host, \
                 port = excluded.port, \
                 username = excluded.username, \
                 from_address = excluded.from_address, \
                 tls_mode = excluded.tls_mode, \
                 updated_at = datetime('now')",
        )
        .bind(&input.host)
        .bind(input.port as i64)
        .bind(input.username.as_deref())
        .bind(&input.from_address)
        .bind(input.tls_mode.as_str())
        .execute(pool)
        .await?;
    }
    Ok(())
}
