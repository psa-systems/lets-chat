// Test role hierarchy: admin > moderator > user
// The actual require_auth/require_role functions need HTTP context,
// so we test the DB-level role operations here.

use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");
    let migration = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(migration)
        .execute(&pool)
        .await
        .expect("Migration failed");
    let migration2 = include_str!("../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(migration2)
        .execute(&pool)
        .await
        .expect("Migration 2 failed");
    let migration3 = include_str!("../migrations/auth/0003_profile_fields.sql");
    sqlx::raw_sql(migration3)
        .execute(&pool)
        .await
        .expect("Migration 3 failed");
    let migration4 = include_str!("../migrations/auth/0004_user_status.sql");
    sqlx::raw_sql(migration4)
        .execute(&pool)
        .await
        .expect("Migration 4 failed");
    let migration5 = include_str!("../migrations/auth/0005_profile_visibility.sql");
    sqlx::raw_sql(migration5)
        .execute(&pool)
        .await
        .expect("Migration 5 failed");
    let migration6 = include_str!("../migrations/auth/0006_user_blocks.sql");
    sqlx::raw_sql(migration6)
        .execute(&pool)
        .await
        .expect("Migration 6 failed");
    let migration7 = include_str!("../migrations/auth/0007_notification_settings.sql");
    sqlx::raw_sql(migration7)
        .execute(&pool)
        .await
        .expect("Migration 7 failed");
    let migration8 = include_str!("../migrations/auth/0008_two_factor.sql");
    sqlx::raw_sql(migration8)
        .execute(&pool)
        .await
        .expect("Migration 8 failed");
    let migration9 = include_str!("../migrations/auth/0009_push_subscriptions.sql");
    sqlx::raw_sql(migration9)
        .execute(&pool)
        .await
        .expect("Migration 9 failed");
    let migration10 = include_str!("../migrations/auth/0010_password_reset.sql");
    sqlx::raw_sql(migration10)
        .execute(&pool)
        .await
        .expect("Migration 10 failed");
    let migration11 = include_str!("../migrations/auth/0011_email_verification.sql");
    sqlx::raw_sql(migration11)
        .execute(&pool)
        .await
        .expect("Migration 11 failed");
    let migration12 = include_str!("../migrations/auth/0012_session_metadata.sql");
    sqlx::raw_sql(migration12)
        .execute(&pool)
        .await
        .expect("Migration 12 failed");
    pool
}

#[tokio::test]
async fn test_default_role_is_user() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    let user = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "user");
}

#[tokio::test]
async fn test_promote_to_moderator() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &id, "moderator")
        .await
        .unwrap();
    let user = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "moderator");
}

#[tokio::test]
async fn test_promote_to_admin() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &id, "admin")
        .await
        .unwrap();
    let user = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "admin");
}

#[tokio::test]
async fn test_demote_admin_to_user() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &id, "admin")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &id, "user")
        .await
        .unwrap();
    let user = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "user");
}
