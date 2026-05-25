use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    let migration = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(migration)
        .execute(&pool)
        .await
        .expect("Failed to run migration");

    let migration2 = include_str!("../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(migration2)
        .execute(&pool)
        .await
        .expect("Failed to run migration 2");

    let migration3 = include_str!("../migrations/auth/0003_profile_fields.sql");
    sqlx::raw_sql(migration3)
        .execute(&pool)
        .await
        .expect("Failed to run migration 3");

    let migration4 = include_str!("../migrations/auth/0004_user_status.sql");
    sqlx::raw_sql(migration4)
        .execute(&pool)
        .await
        .expect("Failed to run migration 4");

    let migration5 = include_str!("../migrations/auth/0005_profile_visibility.sql");
    sqlx::raw_sql(migration5)
        .execute(&pool)
        .await
        .expect("Failed to run migration 5");

    let migration6 = include_str!("../migrations/auth/0006_user_blocks.sql");
    sqlx::raw_sql(migration6)
        .execute(&pool)
        .await
        .expect("Failed to run migration 6");

    let migration7 = include_str!("../migrations/auth/0007_notification_settings.sql");
    sqlx::raw_sql(migration7)
        .execute(&pool)
        .await
        .expect("Failed to run migration 7");

    let migration8 = include_str!("../migrations/auth/0008_two_factor.sql");
    sqlx::raw_sql(migration8)
        .execute(&pool)
        .await
        .expect("Failed to run migration 8");

    let migration9 = include_str!("../migrations/auth/0009_push_subscriptions.sql");
    sqlx::raw_sql(migration9)
        .execute(&pool)
        .await
        .expect("Failed to run migration 9");

    let migration10 = include_str!("../migrations/auth/0010_password_reset.sql");
    sqlx::raw_sql(migration10)
        .execute(&pool)
        .await
        .expect("Failed to run migration 10");

    let migration11 = include_str!("../migrations/auth/0011_email_verification.sql");
    sqlx::raw_sql(migration11)
        .execute(&pool)
        .await
        .expect("Failed to run migration 11");

    let migration12 = include_str!("../migrations/auth/0012_session_metadata.sql");
    sqlx::raw_sql(migration12)
        .execute(&pool)
        .await
        .expect("Failed to run migration 12");

    let migration13 = include_str!("../migrations/auth/0013_digest_columns.sql");
    sqlx::raw_sql(migration13)
        .execute(&pool)
        .await
        .expect("auth migration 13");

    let migration14 = include_str!("../migrations/auth/0014_login_alerts.sql");
    sqlx::raw_sql(migration14)
        .execute(&pool)
        .await
        .expect("auth migration 14");

    let migration15 = include_str!("../migrations/auth/0015_pending_registrations.sql");
    sqlx::raw_sql(migration15)
        .execute(&pool)
        .await
        .expect("auth migration 15");
    let migration16 = include_str!("../migrations/auth/0016_sidebar_categories.sql");
    sqlx::raw_sql(migration16)
        .execute(&pool)
        .await
        .expect("auth migration 16");
    let migration17 =
        include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql");
    sqlx::raw_sql(migration17)
        .execute(&pool)
        .await
        .expect("auth migration 17");
    let migration18 = include_str!("../migrations/auth/0018_starred_rooms.sql");
    sqlx::raw_sql(migration18)
        .execute(&pool)
        .await
        .expect("auth migration 18");
    let migration19 = include_str!("../migrations/auth/0019_api_tokens.sql");
    sqlx::raw_sql(migration19)
        .execute(&pool)
        .await
        .expect("auth migration 19");
    let migration20 = include_str!("../migrations/auth/0020_bots.sql");
    sqlx::raw_sql(migration20)
        .execute(&pool)
        .await
        .expect("auth migration 20");

    let migration21 = include_str!("../migrations/auth/0021_user_dnd.sql");
    sqlx::raw_sql(migration21)
        .execute(&pool)
        .await
        .expect("auth migration 21");

    let migration22 = include_str!("../migrations/auth/0022_mobile_push.sql");
    sqlx::raw_sql(migration22)
        .execute(&pool)
        .await
        .expect("auth migration 22");

    let migration23 = include_str!("../migrations/auth/0023_notify_email_activity.sql");
    sqlx::raw_sql(migration23)
        .execute(&pool)
        .await
        .expect("auth migration 23");

    pool
}

async fn create_test_user(pool: &SqlitePool, username: &str) -> String {
    lets_chat::db::auth::create_user(pool, username, "hash")
        .await
        .expect("Failed to create user")
}

#[tokio::test]
async fn test_create_and_get_invite_code() {
    let pool = setup_pool().await;
    let user_id = create_test_user(&pool, "admin").await;

    lets_chat::db::auth::create_invite_code(&pool, "TESTCODE", &user_id)
        .await
        .expect("create_invite_code should succeed");

    let invite = lets_chat::db::auth::get_invite_code(&pool, "TESTCODE")
        .await
        .expect("get_invite_code should not error");

    assert!(invite.is_some());
    let invite = invite.unwrap();
    assert_eq!(invite.code, "TESTCODE");
    assert_eq!(invite.created_by, user_id);
    assert!(invite.used_by.is_none());
    assert!(invite.used_at.is_none());
}

#[tokio::test]
async fn test_get_missing_invite_code_returns_none() {
    let pool = setup_pool().await;
    let invite = lets_chat::db::auth::get_invite_code(&pool, "NOEXIST")
        .await
        .expect("get_invite_code should not error");
    assert!(invite.is_none());
}

#[tokio::test]
async fn test_list_invite_codes() {
    let pool = setup_pool().await;
    let user_id = create_test_user(&pool, "admin").await;

    lets_chat::db::auth::create_invite_code(&pool, "CODE1", &user_id)
        .await
        .unwrap();
    lets_chat::db::auth::create_invite_code(&pool, "CODE2", &user_id)
        .await
        .unwrap();

    let codes = lets_chat::db::auth::list_invite_codes(&pool)
        .await
        .expect("list_invite_codes should not error");

    assert_eq!(codes.len(), 2);
}

#[tokio::test]
async fn test_use_invite_code() {
    let pool = setup_pool().await;
    let admin_id = create_test_user(&pool, "admin").await;
    let user_id = create_test_user(&pool, "newuser").await;

    lets_chat::db::auth::create_invite_code(&pool, "USETEST", &admin_id)
        .await
        .unwrap();

    lets_chat::db::auth::redeem_invite_code(&pool, "USETEST", &user_id)
        .await
        .expect("redeem_invite_code should succeed");

    let invite = lets_chat::db::auth::get_invite_code(&pool, "USETEST")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(invite.used_by, Some(user_id));
    assert!(invite.used_at.is_some());
}

#[tokio::test]
async fn test_revoke_invite_code() {
    let pool = setup_pool().await;
    let user_id = create_test_user(&pool, "admin").await;

    let code_id = lets_chat::db::auth::create_invite_code(&pool, "REVOKE", &user_id)
        .await
        .unwrap();

    lets_chat::db::auth::revoke_invite_code(&pool, code_id)
        .await
        .expect("revoke_invite_code should succeed");

    let invite = lets_chat::db::auth::get_invite_code(&pool, "REVOKE")
        .await
        .unwrap();
    assert!(invite.is_none());
}

#[tokio::test]
async fn test_duplicate_invite_code_fails() {
    let pool = setup_pool().await;
    let user_id = create_test_user(&pool, "admin").await;

    lets_chat::db::auth::create_invite_code(&pool, "DUPE", &user_id)
        .await
        .expect("First create should succeed");

    let result = lets_chat::db::auth::create_invite_code(&pool, "DUPE", &user_id).await;
    assert!(result.is_err());
}
