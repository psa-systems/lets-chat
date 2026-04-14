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
