#[cfg(not(target_arch = "wasm32"))]
pub mod auth;
#[cfg(not(target_arch = "wasm32"))]
pub mod chat;
#[cfg(not(target_arch = "wasm32"))]
pub mod moderation;
#[cfg(not(target_arch = "wasm32"))]
pub mod settings;

#[cfg(not(target_arch = "wasm32"))]
use sqlx::SqlitePool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

#[cfg(not(target_arch = "wasm32"))]
static CHAT_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(not(target_arch = "wasm32"))]
static AUTH_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(not(target_arch = "wasm32"))]
static SETTINGS_POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();

#[cfg(not(target_arch = "wasm32"))]
static DATA_DIR: OnceLock<String> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub fn set_data_dir(dir: String) {
    DATA_DIR.set(dir).expect("data dir already set");
}

#[cfg(not(target_arch = "wasm32"))]
fn data_dir() -> &'static str {
    DATA_DIR.get().map(|s| s.as_str()).unwrap_or("/data")
}

#[cfg(not(target_arch = "wasm32"))]
async fn init_chat_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/chat.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to chat DB");

    let migration_sql = include_str!("../../migrations/chat/0001_create_tables.sql");
    sqlx::raw_sql(migration_sql)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration");

    let migration_002 = include_str!("../../migrations/chat/0002_moderation.sql");
    sqlx::raw_sql(migration_002)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration 002");

    let migration_003 = include_str!("../../migrations/chat/0003_dms.sql");
    sqlx::raw_sql(migration_003)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration 003");

    let migration_004 = include_str!("../../migrations/chat/0004_message_editing.sql");
    sqlx::raw_sql(migration_004)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration 004");

    let migration_005 = include_str!("../../migrations/chat/0005_private_rooms.sql");
    sqlx::raw_sql(migration_005)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration 005");

    let migration_006 = include_str!("../../migrations/chat/0006_read_receipts.sql");
    sqlx::raw_sql(migration_006)
        .execute(&pool)
        .await
        .expect("Failed to run chat DB migration 006");

    pool
}

#[cfg(not(target_arch = "wasm32"))]
async fn init_auth_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/auth.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to auth DB");

    let migration_sql = include_str!("../../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(migration_sql)
        .execute(&pool)
        .await
        .expect("Failed to run auth DB migration");

    let auth_m2 = include_str!("../../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(auth_m2)
        .execute(&pool)
        .await
        .expect("Failed to run auth DB migration 002");

    pool
}

#[cfg(not(target_arch = "wasm32"))]
async fn init_settings_pool() -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");

    let pool = SqlitePool::connect(&format!("sqlite:{}/settings.db?mode=rwc", dir))
        .await
        .expect("Failed to connect to settings DB");

    let migration_sql = include_str!("../../migrations/settings/0001_create_tables.sql");
    sqlx::raw_sql(migration_sql)
        .execute(&pool)
        .await
        .expect("Failed to run settings DB migration");

    pool
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn get_chat_pool() -> &'static SqlitePool {
    CHAT_POOL.get_or_init(init_chat_pool).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn get_auth_pool() -> &'static SqlitePool {
    AUTH_POOL.get_or_init(init_auth_pool).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn get_settings_pool() -> &'static SqlitePool {
    SETTINGS_POOL.get_or_init(init_settings_pool).await
}
