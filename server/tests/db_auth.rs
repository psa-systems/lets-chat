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

    pool
}

#[tokio::test]
async fn test_create_user_and_find_by_username() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hashed_pw_placeholder")
        .await
        .expect("Failed to create user");
    assert!(!user_id.is_empty());

    let found = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .expect("Failed to find user");
    assert!(found.is_some());
    let user = found.unwrap();
    assert_eq!(user.username, "alice");
    assert_eq!(user.role, "user");
    assert!(!user.is_banned);
}

#[tokio::test]
async fn test_create_user_duplicate_username_fails() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .expect("First create should succeed");
    let result = lets_chat::db::auth::create_user(&pool, "alice", "hash2").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_username_case_insensitive() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "Alice", "hash1")
        .await
        .expect("Create should succeed");
    let found = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .expect("Lookup should succeed");
    assert!(found.is_some());
    assert_eq!(found.unwrap().username, "Alice");
}

#[tokio::test]
async fn test_count_users() {
    let pool = setup_pool().await;
    let count = lets_chat::db::auth::count_users(&pool)
        .await
        .expect("Count should work");
    assert_eq!(count, 0);
    lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let count = lets_chat::db::auth::count_users(&pool)
        .await
        .expect("Count should work");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_set_user_role() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &user_id, "admin")
        .await
        .expect("Set role should work");
    let user = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "admin");
}

#[tokio::test]
async fn test_create_and_validate_session() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = lets_chat::db::auth::create_session(&pool, &user_id)
        .await
        .expect("Create session should work");
    assert!(!session_id.is_empty());
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .expect("Get session user should work");
    assert!(session_user.is_some());
    assert_eq!(session_user.unwrap().username, "alice");
}

#[tokio::test]
async fn test_delete_session() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = lets_chat::db::auth::create_session(&pool, &user_id)
        .await
        .unwrap();
    lets_chat::db::auth::delete_session(&pool, &session_id)
        .await
        .expect("Delete session should work");
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .unwrap();
    assert!(session_user.is_none());
}

#[tokio::test]
async fn test_expired_session_returns_none() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, datetime('now', '-1 hour'))",
    )
    .bind(&session_id)
    .bind(&user_id)
    .execute(&pool)
    .await
    .unwrap();
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .unwrap();
    assert!(session_user.is_none());
}
