use sqlx::SqlitePool;

mod common;

async fn setup_pools() -> (SqlitePool, SqlitePool) {
    (common::auth_pool().await, common::chat_pool().await)
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

// LC-535: the 60s background sweep nulls a timed mute once its expiry passes,
// but never touches a permanent (NULL muted_until) or still-future mute.
#[tokio::test]
async fn test_clear_expired_mutes_lifts_past_mute() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let id = lets_chat::db::auth::create_user(&auth_pool, "erin", "hash")
        .await
        .expect("create user");
    lets_chat::db::auth::mute_user(
        &auth_pool,
        &id,
        Some("2000-01-01 00:00:00"),
        Some("timeout"),
    )
    .await
    .expect("mute user");

    let cleared = lets_chat::db::auth::clear_expired_mutes(&auth_pool)
        .await
        .expect("clear expired mutes");
    assert_eq!(cleared, 1, "one expired mute should be cleared");

    let user = lets_chat::db::auth::find_user_by_id(&auth_pool, &id)
        .await
        .expect("find user")
        .expect("user should exist");
    assert!(!user.is_muted, "expired mute is lifted");
    assert_eq!(user.muted_until, None);
    assert_eq!(user.mute_reason, None);
}

#[tokio::test]
async fn test_clear_expired_mutes_spares_permanent_and_future() {
    let (auth_pool, _chat_pool) = setup_pools().await;

    let perm = lets_chat::db::auth::create_user(&auth_pool, "frank", "hash")
        .await
        .expect("create perm user");
    lets_chat::db::auth::mute_user(&auth_pool, &perm, None, None)
        .await
        .expect("permanent mute");

    let future = lets_chat::db::auth::create_user(&auth_pool, "grace", "hash")
        .await
        .expect("create future user");
    lets_chat::db::auth::mute_user(&auth_pool, &future, Some("2099-12-31 23:59:59"), None)
        .await
        .expect("future mute");

    let cleared = lets_chat::db::auth::clear_expired_mutes(&auth_pool)
        .await
        .expect("clear expired mutes");
    assert_eq!(cleared, 0, "neither permanent nor future mute expires");

    for id in [&perm, &future] {
        let user = lets_chat::db::auth::find_user_by_id(&auth_pool, id)
            .await
            .expect("find user")
            .expect("user should exist");
        assert!(user.is_muted, "mute {id} should still be in force");
    }
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
