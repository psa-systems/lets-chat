use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    sqlx::raw_sql(include_str!("../migrations/chat/0001_create_tables.sql"))
        .execute(&pool)
        .await
        .expect("migration 1");
    sqlx::raw_sql(include_str!("../migrations/chat/0002_moderation.sql"))
        .execute(&pool)
        .await
        .expect("migration 2");
    sqlx::raw_sql(include_str!("../migrations/chat/0003_dms.sql"))
        .execute(&pool)
        .await
        .expect("migration 3");
    sqlx::raw_sql(include_str!("../migrations/chat/0004_message_editing.sql"))
        .execute(&pool)
        .await
        .expect("migration 4");
    sqlx::raw_sql(include_str!("../migrations/chat/0005_private_rooms.sql"))
        .execute(&pool)
        .await
        .expect("migration 5");
    sqlx::raw_sql(include_str!("../migrations/chat/0006_read_receipts.sql"))
        .execute(&pool)
        .await
        .expect("migration 6");
    sqlx::raw_sql(include_str!("../migrations/chat/0007_reactions.sql"))
        .execute(&pool)
        .await
        .expect("migration 7");
    sqlx::raw_sql(include_str!("../migrations/chat/0008_search.sql"))
        .execute(&pool)
        .await
        .expect("migration 8");
    sqlx::raw_sql(include_str!("../migrations/chat/0009_enclaves.sql"))
        .execute(&pool)
        .await
        .expect("migration 9");
    sqlx::raw_sql(include_str!("../migrations/chat/0014_mentions.sql"))
        .execute(&pool)
        .await
        .expect("migration 14");
    sqlx::raw_sql(include_str!(
        "../migrations/chat/0015_room_notification_settings.sql"
    ))
    .execute(&pool)
    .await
    .expect("migration 15");
    sqlx::raw_sql(include_str!(
        "../migrations/chat/0024_voice_channel_flag.sql"
    ))
    .execute(&pool)
    .await
    .expect("migration 24");

    pool
}

#[tokio::test]
async fn test_list_rooms_excludes_private_for_non_member() {
    let pool = setup_pool().await;

    // Create a private room
    lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("invite-abc"), None)
        .await
        .unwrap();

    // Non-member should not see the private room
    let rooms = lets_chat::db::chat::list_rooms(&pool, "user-1", false)
        .await
        .unwrap();
    assert!(
        rooms.iter().all(|r| r.room_type != "private"),
        "non-member should not see private rooms"
    );
}

#[tokio::test]
async fn test_list_rooms_includes_private_for_member() {
    let pool = setup_pool().await;

    let room_id = lets_chat::db::chat::create_room(
        &pool,
        "secret",
        None,
        "private",
        Some("invite-abc"),
        None,
    )
    .await
    .unwrap();
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();

    // Member should see the private room
    let rooms = lets_chat::db::chat::list_rooms(&pool, "user-1", false)
        .await
        .unwrap();
    assert!(
        rooms.iter().any(|r| r.name == "secret"),
        "member should see private room they joined"
    );
}

#[tokio::test]
async fn test_admin_sees_all_rooms_including_private() {
    let pool = setup_pool().await;

    lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("invite-abc"), None)
        .await
        .unwrap();

    // Admin sees all — not a member but is_admin = true
    let rooms = lets_chat::db::chat::list_rooms(&pool, "admin-user", true)
        .await
        .unwrap();
    assert!(
        rooms.iter().any(|r| r.name == "secret"),
        "admin should see private rooms regardless of membership"
    );
}

#[tokio::test]
async fn test_get_room_by_invite_code() {
    let pool = setup_pool().await;

    lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("invite-xyz"), None)
        .await
        .unwrap();

    let found = lets_chat::db::chat::get_room_by_invite(&pool, "invite-xyz")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "secret");

    let not_found = lets_chat::db::chat::get_room_by_invite(&pool, "wrong-code")
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_is_room_member() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("code"), None)
            .await
            .unwrap();

    assert!(
        !lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );

    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    assert!(
        lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );

    lets_chat::db::chat::remove_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    assert!(
        !lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_add_room_member_is_idempotent() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("code"), None)
            .await
            .unwrap();

    // Adding same user twice should not error (INSERT OR IGNORE)
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();
    lets_chat::db::chat::add_room_member(&pool, room_id, "user-1")
        .await
        .unwrap();

    assert!(
        lets_chat::db::chat::is_room_member(&pool, room_id, "user-1")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_regenerate_invite_code() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", Some("old-code"), None)
            .await
            .unwrap();

    lets_chat::db::chat::regenerate_invite_code(&pool, room_id, "new-code")
        .await
        .unwrap();

    // Old code should no longer work
    let by_old = lets_chat::db::chat::get_room_by_invite(&pool, "old-code")
        .await
        .unwrap();
    assert!(by_old.is_none());

    // New code should work
    let by_new = lets_chat::db::chat::get_room_by_invite(&pool, "new-code")
        .await
        .unwrap();
    assert!(by_new.is_some());
    assert_eq!(by_new.unwrap().id, room_id);
}
