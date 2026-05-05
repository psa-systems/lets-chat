pub mod auth;
pub mod chat;
pub mod enclave;
pub mod moderation;
pub mod settings;

use sqlx::SqlitePool;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<String> = OnceLock::new();

pub fn set_data_dir(dir: String) {
    DATA_DIR.set(dir).expect("data dir already set");
}

fn data_dir() -> &'static str {
    DATA_DIR.get().map(|s| s.as_str()).unwrap_or("/data")
}

async fn init_pool(name: &str, migrator: sqlx::migrate::Migrator) -> SqlitePool {
    let dir = data_dir();
    std::fs::create_dir_all(dir).expect("Failed to create data directory");
    let pool = SqlitePool::connect(&format!("sqlite:{}/{}.db?mode=rwc", dir, name))
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
