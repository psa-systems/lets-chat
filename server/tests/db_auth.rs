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

#[tokio::test]
async fn test_search_users_matches_username_substring_case_insensitive() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "Alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "carol", "h")
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].username, "Alice");
}

#[tokio::test]
async fn test_search_users_matches_display_name() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "u1", "h")
        .await
        .unwrap();
    lets_chat::db::auth::update_user_profile(&pool, &id, Some("Jane Doe"), None)
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "other", "h")
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "jane", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);
}

#[tokio::test]
async fn test_search_users_excludes_banned() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::ban_user(&pool, &id, Some("spam"))
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn test_search_users_escapes_like_wildcards() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();

    // `%` would match every row if not escaped; with escaping it matches nothing.
    let hits = lets_chat::db::auth::search_users(&pool, "%", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());

    let hits = lets_chat::db::auth::search_users(&pool, "_", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn test_search_users_respects_limit() {
    let pool = setup_pool().await;
    for n in 0..5 {
        lets_chat::db::auth::create_user(&pool, &format!("user{n}"), "h")
            .await
            .unwrap();
    }
    let hits = lets_chat::db::auth::search_users(&pool, "user", "viewer", 3)
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
}

#[tokio::test]
async fn test_search_users_excludes_private_profiles() {
    let pool = setup_pool().await;
    let alice_id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let _bob_id = lets_chat::db::auth::create_user(&pool, "alicia", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_profile_public(&pool, &alice_id, false)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].username, "alicia");
}

#[tokio::test]
async fn test_block_and_unblock_round_trip() {
    let pool = setup_pool().await;
    let a = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let b = lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();

    assert!(!lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(!lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());

    lets_chat::db::auth::block_user(&pool, &a, &b)
        .await
        .unwrap();
    assert!(lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());
    assert!(lets_chat::db::auth::is_blocked_either_way(&pool, &b, &a)
        .await
        .unwrap());

    // Idempotent.
    lets_chat::db::auth::block_user(&pool, &a, &b)
        .await
        .unwrap();
    let blocked = lets_chat::db::auth::list_blocked_users(&pool, &a)
        .await
        .unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].id, b);

    lets_chat::db::auth::unblock_user(&pool, &a, &b)
        .await
        .unwrap();
    assert!(!lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(!lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());
}

#[tokio::test]
async fn test_list_blocked_ids_either_way_includes_both_directions() {
    let pool = setup_pool().await;
    let viewer = lets_chat::db::auth::create_user(&pool, "viewer", "h")
        .await
        .unwrap();
    let blocked_by_viewer = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let blocks_viewer = lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();
    let unrelated = lets_chat::db::auth::create_user(&pool, "carol", "h")
        .await
        .unwrap();

    lets_chat::db::auth::block_user(&pool, &viewer, &blocked_by_viewer)
        .await
        .unwrap();
    lets_chat::db::auth::block_user(&pool, &blocks_viewer, &viewer)
        .await
        .unwrap();

    let ids = lets_chat::db::auth::list_blocked_ids_either_way(&pool, &viewer)
        .await
        .unwrap();
    assert!(ids.contains(&blocked_by_viewer));
    assert!(ids.contains(&blocks_viewer));
    assert!(!ids.contains(&unrelated));
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn test_search_excludes_blocked_either_direction() {
    let pool = setup_pool().await;
    let viewer = lets_chat::db::auth::create_user(&pool, "viewer", "h")
        .await
        .unwrap();
    let blocked_by_viewer = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let blocks_viewer = lets_chat::db::auth::create_user(&pool, "alicia", "h")
        .await
        .unwrap();
    let _other = lets_chat::db::auth::create_user(&pool, "alma", "h")
        .await
        .unwrap();

    lets_chat::db::auth::block_user(&pool, &viewer, &blocked_by_viewer)
        .await
        .unwrap();
    lets_chat::db::auth::block_user(&pool, &blocks_viewer, &viewer)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "al", &viewer, 50)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|u| u.username.as_str()).collect();
    assert_eq!(names, vec!["alma"]);
}

#[tokio::test]
async fn test_search_users_self_visible_when_private() {
    let pool = setup_pool().await;
    let alice_id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_profile_public(&pool, &alice_id, false)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", &alice_id, 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, alice_id);
}
