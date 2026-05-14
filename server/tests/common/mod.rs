//! Shared test helpers.
//!
//! Integration tests need in-memory SQLite pools with the schema applied.
//! Each test file used to hand-list `include_str!` migrations, which silently
//! went stale every time a migration was added (e.g. `messages.quote_id`).
//! These helpers run the full embedded migration set via `sqlx::migrate!`, so
//! new migrations are picked up automatically at compile time.
//!
//! Not every test binary uses every helper, so suppress dead-code warnings
//! for the module as a whole rather than annotating each function.
#![allow(dead_code)]

use sqlx::SqlitePool;

/// In-memory `auth.db` pool with all auth migrations applied.
pub async fn auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    sqlx::migrate!("./migrations/auth")
        .run(&pool)
        .await
        .expect("auth migrations");
    pool
}

/// In-memory `chat.db` pool with all chat migrations applied.
pub async fn chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    sqlx::migrate!("./migrations/chat")
        .run(&pool)
        .await
        .expect("chat migrations");
    pool
}

/// In-memory `settings.db` pool with all settings migrations applied.
pub async fn settings_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("settings pool");
    sqlx::migrate!("./migrations/settings")
        .run(&pool)
        .await
        .expect("settings migrations");
    pool
}

/// Open a pool by domain name (`"auth"`, `"chat"`, `"settings"`).
pub async fn pool(name: &str) -> SqlitePool {
    match name {
        "auth" => auth_pool().await,
        "chat" => chat_pool().await,
        "settings" => settings_pool().await,
        other => panic!("unknown db domain: {other}"),
    }
}
