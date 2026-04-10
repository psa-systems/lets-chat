use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    let migration = include_str!("../migrations/settings/0001_create_tables.sql");
    sqlx::raw_sql(migration)
        .execute(&pool)
        .await
        .expect("Failed to run migration");

    pool
}

#[tokio::test]
async fn test_get_default_setting() {
    let pool = setup_pool().await;
    let value = lets_chat::db::settings::get_setting(&pool, "site_name")
        .await
        .expect("get_setting should not error");
    assert_eq!(value, Some("Let's Chat".to_string()));
}

#[tokio::test]
async fn test_get_missing_setting_returns_none() {
    let pool = setup_pool().await;
    let value = lets_chat::db::settings::get_setting(&pool, "nonexistent_key")
        .await
        .expect("get_setting should not error");
    assert!(value.is_none());
}

#[tokio::test]
async fn test_set_and_get_setting() {
    let pool = setup_pool().await;
    lets_chat::db::settings::set_setting(&pool, "site_name", "My Chat")
        .await
        .expect("set_setting should succeed");

    let value = lets_chat::db::settings::get_setting(&pool, "site_name")
        .await
        .expect("get_setting should not error");
    assert_eq!(value, Some("My Chat".to_string()));
}

#[tokio::test]
async fn test_set_new_setting() {
    let pool = setup_pool().await;
    lets_chat::db::settings::set_setting(&pool, "custom_key", "custom_value")
        .await
        .expect("set_setting should succeed for new keys");

    let value = lets_chat::db::settings::get_setting(&pool, "custom_key")
        .await
        .expect("get_setting should not error");
    assert_eq!(value, Some("custom_value".to_string()));
}

#[tokio::test]
async fn test_get_all_settings_returns_defaults() {
    let pool = setup_pool().await;
    let all = lets_chat::db::settings::get_all_settings(&pool)
        .await
        .expect("get_all_settings should not error");

    assert!(!all.is_empty());

    // Verify some known defaults are present
    let site_name = all.iter().find(|(k, _)| k == "site_name");
    assert!(site_name.is_some());
    assert_eq!(site_name.unwrap().1, "Let's Chat");

    let registration_open = all.iter().find(|(k, _)| k == "registration_open");
    assert!(registration_open.is_some());
    assert_eq!(registration_open.unwrap().1, "true");
}

#[tokio::test]
async fn test_set_setting_overwrites() {
    let pool = setup_pool().await;

    lets_chat::db::settings::set_setting(&pool, "registration_open", "false")
        .await
        .expect("First set should succeed");

    lets_chat::db::settings::set_setting(&pool, "registration_open", "true")
        .await
        .expect("Second set should succeed");

    let value = lets_chat::db::settings::get_setting(&pool, "registration_open")
        .await
        .expect("get_setting should not error");
    assert_eq!(value, Some("true".to_string()));
}
