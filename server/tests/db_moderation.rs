use sqlx::SqlitePool;

async fn setup_pools() -> (SqlitePool, SqlitePool) {
    let auth_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    let auth_migration = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(auth_migration)
        .execute(&auth_pool)
        .await
        .expect("auth migration");
    let auth_m2 = include_str!("../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(auth_m2)
        .execute(&auth_pool)
        .await
        .expect("auth migration 2");

    let chat_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    let chat_m1 = include_str!("../migrations/chat/0001_create_tables.sql");
    sqlx::raw_sql(chat_m1)
        .execute(&chat_pool)
        .await
        .expect("chat migration 1");
    let chat_m2 = include_str!("../migrations/chat/0002_moderation.sql");
    sqlx::raw_sql(chat_m2)
        .execute(&chat_pool)
        .await
        .expect("chat migration 2");

    let chat_m3 = include_str!("../migrations/chat/0003_dms.sql");
    sqlx::raw_sql(chat_m3)
        .execute(&chat_pool)
        .await
        .expect("chat migration 3");

    let chat_m4 = include_str!("../migrations/chat/0004_message_editing.sql");
    sqlx::raw_sql(chat_m4)
        .execute(&chat_pool)
        .await
        .expect("chat migration 4");

    let chat_m5 = include_str!("../migrations/chat/0005_private_rooms.sql");
    sqlx::raw_sql(chat_m5)
        .execute(&chat_pool)
        .await
        .expect("chat migration 5");

    let chat_m6 = include_str!("../migrations/chat/0006_read_receipts.sql");
    sqlx::raw_sql(chat_m6)
        .execute(&chat_pool)
        .await
        .expect("chat migration 6");

    (auth_pool, chat_pool)
}

#[tokio::test]
async fn test_ban_user() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let id = lets_chat::db::auth::create_user(&auth_pool, "alice", "hash")
        .await
        .expect("create user");

    lets_chat::db::auth::ban_user(&auth_pool, &id, Some("spam"))
        .await
        .expect("ban user");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");

    assert!(user.is_banned);
    assert_eq!(user.ban_reason, Some("spam".to_string()));
    assert_eq!(user.banned_until, None);
}

#[tokio::test]
async fn test_unban_user() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let id = lets_chat::db::auth::create_user(&auth_pool, "bob", "hash")
        .await
        .expect("create user");

    lets_chat::db::auth::ban_user(&auth_pool, &id, Some("spam"))
        .await
        .expect("ban user");

    lets_chat::db::auth::unban_user(&auth_pool, &id)
        .await
        .expect("unban user");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");

    assert!(!user.is_banned);
    assert_eq!(user.ban_reason, None);
}

#[tokio::test]
async fn test_suspend_user() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let id = lets_chat::db::auth::create_user(&auth_pool, "carol", "hash")
        .await
        .expect("create user");

    lets_chat::db::auth::suspend_user(&auth_pool, &id, "2099-12-31 23:59:59", Some("timeout"))
        .await
        .expect("suspend user");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");

    assert!(user.is_banned);
    assert_eq!(user.ban_reason, Some("timeout".to_string()));
    assert_eq!(user.banned_until, Some("2099-12-31 23:59:59".to_string()));
}

#[tokio::test]
async fn test_mute_and_unmute_user() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let id = lets_chat::db::auth::create_user(&auth_pool, "dave", "hash")
        .await
        .expect("create user");

    lets_chat::db::auth::mute_user(
        &auth_pool,
        &id,
        Some("2099-12-31 23:59:59"),
        Some("cool down"),
    )
    .await
    .expect("mute user");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");

    assert!(user.is_muted);
    assert_eq!(user.mute_reason, Some("cool down".to_string()));

    lets_chat::db::auth::unmute_user(&auth_pool, &id)
        .await
        .expect("unmute user");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");

    assert!(!user.is_muted);
    assert_eq!(user.mute_reason, None);
}

#[tokio::test]
async fn test_log_and_list_mod_actions() {
    let (_auth_pool, chat_pool) = setup_pools().await;

    lets_chat::db::moderation::log_mod_action(
        &chat_pool,
        "ban",
        "user-1",
        "admin-1",
        Some("spam"),
        None,
        None,
    )
    .await
    .expect("log ban action");

    lets_chat::db::moderation::log_mod_action(
        &chat_pool,
        "mute",
        "user-1",
        "admin-1",
        Some("spam"),
        None,
        None,
    )
    .await
    .expect("log mute action");

    let actions = lets_chat::db::moderation::list_mod_actions(&chat_pool)
        .await
        .expect("list mod actions");

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].action, "mute");
    assert_eq!(actions[1].action, "ban");
}

#[tokio::test]
async fn test_soft_delete_message() {
    let (_auth_pool, chat_pool) = setup_pools().await;

    let msg_id = lets_chat::db::chat::insert_message(&chat_pool, 1, "user-1", "hello")
        .await
        .expect("insert message");

    let messages = lets_chat::db::chat::list_messages(&chat_pool, 1)
        .await
        .expect("list messages before delete");
    assert_eq!(messages.len(), 1);

    lets_chat::db::moderation::soft_delete_message(&chat_pool, msg_id, "mod-1")
        .await
        .expect("soft delete message");

    let messages = lets_chat::db::chat::list_messages(&chat_pool, 1)
        .await
        .expect("list messages after delete");
    assert_eq!(messages.len(), 0);
}
