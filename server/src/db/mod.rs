pub mod auth;
pub mod bookmarks;
pub mod chat;
pub mod custom_emojis;
pub mod email_verification;
pub mod enclave;
pub mod mentions;
pub mod moderation;
pub mod notifications;
pub mod password_reset;
pub mod pinned;
pub mod push_subscriptions;
pub mod settings;
pub mod two_factor;
pub mod uploads;
pub mod vapid;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

static DATA_DIR: OnceLock<String> = OnceLock::new();

/// Initialize the global data directory. Called once at startup from main.
/// Idempotent for tests: a second call with the same string is a no-op; a
/// second call with a different string is also silently ignored, since the
/// first writer wins (`OnceLock` semantics) and the production caller is
/// always main().
pub fn set_data_dir(dir: String) {
    let _ = DATA_DIR.set(dir);
}

fn data_dir() -> &'static str {
    DATA_DIR.get().map(|s| s.as_str()).unwrap_or("/data")
}

/// Directory where user avatar files live. Created on demand.
pub fn avatars_dir() -> PathBuf {
    let p = PathBuf::from(data_dir()).join("avatars");
    if let Err(e) = std::fs::create_dir_all(&p) {
        tracing::warn!(error = %e, path = %p.display(), "failed to create avatars dir");
    }
    p
}

/// Directory where user-uploaded attachments live (content-addressed by sha256).
pub fn uploads_dir() -> PathBuf {
    let p = PathBuf::from(data_dir()).join("uploads");
    if let Err(e) = std::fs::create_dir_all(&p) {
        tracing::warn!(error = %e, path = %p.display(), "failed to create uploads dir");
    }
    p
}

async fn init_pool(name: &str, migrator: sqlx::migrate::Migrator) -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");
    // WAL allows readers to proceed while a writer holds the file, and
    // `busy_timeout` makes contending writers wait for the lock instead
    // of immediately returning SQLITE_BUSY. Without these the auth pool
    // hits "database is locked" any time activity-touch, session
    // last-seen, and a normal write collide in the same instant.
    let url = format!("sqlite:{}/{}.db", dir, name);
    let opts = SqliteConnectOptions::from_str(&url)
        .unwrap_or_else(|e| panic!("invalid {} db url: {}", name, e))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));
    // Pre-open a small set of connections so the first burst of requests
    // does not pay the SQLite open + PRAGMA cost serially, and shorten
    // the acquire timeout so a saturated pool surfaces as a fast 500 in
    // the logs instead of stalling the response for the default 30 s.
    let pool = SqlitePoolOptions::new()
        .max_connections(16)
        .min_connections(4)
        .acquire_timeout(Duration::from_secs(3))
        .test_before_acquire(false)
        .connect_with(opts)
        .await
        .unwrap_or_else(|e| panic!("Failed to connect to {} DB: {}", name, e));
    migrator
        .run(&pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to run {} migrations: {}", name, e));
    pool
}

pub async fn open_chat_pool() -> SqlitePool {
    init_pool("chat", sqlx::migrate!("./migrations/chat")).await
}

pub async fn open_auth_pool() -> SqlitePool {
    init_pool("auth", sqlx::migrate!("./migrations/auth")).await
}

pub async fn open_settings_pool() -> SqlitePool {
    init_pool("settings", sqlx::migrate!("./migrations/settings")).await
}
