#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod chat;
#[cfg(feature = "server")]
pub mod moderation;
#[cfg(feature = "server")]
pub mod settings;

#[cfg(feature = "server")]
use sqlx::SqlitePool;
#[cfg(feature = "server")]
use std::sync::OnceLock;

#[cfg(feature = "server")]
static CHAT_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(feature = "server")]
static AUTH_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(feature = "server")]
static SETTINGS_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(feature = "server")]
static DATA_DIR: OnceLock<String> = OnceLock::new();

#[cfg(feature = "server")]
pub fn set_data_dir(dir: String) {
    DATA_DIR.set(dir).expect("data dir already set");
}

#[cfg(feature = "server")]
fn data_dir() -> &'static str {
    DATA_DIR.get().map(|s| s.as_str()).unwrap_or("/data")
}

#[cfg(feature = "server")]
async fn init_chat_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/chat.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to chat DB");

    sqlx::migrate!("./migrations/chat")
        .run(&pool)
        .await
        .expect("Failed to run chat DB migrations");

    pool
}

#[cfg(feature = "server")]
async fn init_auth_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/auth.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to auth DB");

    sqlx::migrate!("./migrations/auth")
        .run(&pool)
        .await
        .expect("Failed to run auth DB migrations");

    pool
}

#[cfg(feature = "server")]
async fn init_settings_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/settings.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to settings DB");

    sqlx::migrate!("./migrations/settings")
        .run(&pool)
        .await
        .expect("Failed to run settings DB migrations");

    pool
}

#[cfg(feature = "server")]
pub async fn get_chat_pool() -> &'static SqlitePool {
    CHAT_POOL.get_or_init(init_chat_pool).await
}

#[cfg(feature = "server")]
pub async fn get_auth_pool() -> &'static SqlitePool {
    AUTH_POOL.get_or_init(init_auth_pool).await
}

#[cfg(feature = "server")]
pub async fn get_settings_pool() -> &'static SqlitePool {
    SETTINGS_POOL.get_or_init(init_settings_pool).await
}
