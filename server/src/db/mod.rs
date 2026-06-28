pub mod acks;
pub mod activity;
pub mod analytics;
pub mod anti_spam;
pub mod api_tokens;
pub mod apns_subscriptions;
pub mod auth;
pub mod bookmarks;
pub mod branding;
pub mod bridge_avatar_proxies;
pub mod bridges;
pub mod chat;
pub mod custom_emojis;
pub mod drafts;
pub mod email_inbox;
pub mod email_ingress_dedup;
pub mod email_ingress_drops;
pub mod enclave;
pub mod fcm_subscriptions;
pub mod imap_config;
pub mod imap_poll_status;
pub mod inbox;
pub mod mentions;
pub mod moderation;
pub mod notification_keywords;
pub mod notifications;
pub mod oidc_pending;
pub mod outgoing_webhooks;
pub mod pinned;
pub mod polls;
pub mod push_subscriptions;
pub mod quota;
pub mod reminders;
pub mod remote_control_audit;
pub mod reply_tokens;
pub mod reports;
pub mod retention_status;
pub mod room_feeds;
pub mod room_nicknames;
pub mod room_rbac;
pub mod saved_searches;
pub mod scheduled;
pub mod settings;
pub mod shame_tags;
pub mod sidebar_categories;
pub mod slash;
pub mod starred_rooms;
pub mod thread_followers;
pub mod transcripts;
pub mod translations;
pub mod uploads;
pub mod user_groups;
pub mod vapid;
pub mod webhooks;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

/// LC-147: maximum active push subscriptions a single user may hold *per
/// channel* (Web Push, APNs, FCM each get their own budget). Registering
/// beyond this evicts the least-recently-seen rows for that user in that
/// channel. Generous for real multi-device use; far below abuse levels. The
/// register endpoints require an authenticated session, so this caps a
/// logged-in user's storage footprint rather than an anonymous one.
pub const MAX_PUSH_SUBSCRIPTIONS_PER_USER: i64 = 20;

static DATA_DIR: OnceLock<String> = OnceLock::new();

/// Initialize the global data directory. Called once at startup from main.
/// Idempotent for tests: a second call with the same string is a no-op; a
/// second call with a different string is also silently ignored, since the
/// first writer wins (`OnceLock` semantics) and the production caller is
/// always main().
pub fn set_data_dir(dir: String) {
    let _ = DATA_DIR.set(dir);
}

/// The currently active data directory. Exposed so the backup /
/// restore admin routes can locate sibling staging paths and the
/// marker file without re-implementing the env-var + default chain.
pub fn data_dir() -> &'static str {
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

/// LC-78-AVATAR-PROXY: directory where bridge-avatar cache files live,
/// named `{hash}` (no extension; content type comes from
/// `bridge_avatar_proxies.content_type`). One file per row in the cache
/// table; the GC sweep deletes both together.
pub fn bridge_avatars_dir() -> PathBuf {
    let p = PathBuf::from(data_dir()).join("bridge-avatars");
    if let Err(e) = std::fs::create_dir_all(&p) {
        tracing::warn!(error = %e, path = %p.display(), "failed to create bridge-avatars dir");
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
    let url = format!("sqlite:{dir}/{name}.db");
    let opts = SqliteConnectOptions::from_str(&url)
        .unwrap_or_else(|e| panic!("invalid {name} db url: {e}"))
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
        .unwrap_or_else(|e| panic!("Failed to connect to {name} DB: {e}"));
    migrator
        .run(&pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to run {name} migrations: {e}"));
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
