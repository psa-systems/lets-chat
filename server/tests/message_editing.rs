use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    let m1 = include_str!("../migrations/chat/0001_create_tables.sql");
    sqlx::raw_sql(m1).execute(&pool).await.expect("migration 1");

    let m2 = include_str!("../migrations/chat/0002_moderation.sql");
    sqlx::raw_sql(m2).execute(&pool).await.expect("migration 2");

    let m3 = include_str!("../migrations/chat/0003_dms.sql");
    sqlx::raw_sql(m3).execute(&pool).await.expect("migration 3");

    let m4 = include_str!("../migrations/chat/0004_message_editing.sql");
    sqlx::raw_sql(m4).execute(&pool).await.expect("migration 4");

    let m5 = include_str!("../migrations/chat/0005_private_rooms.sql");
    sqlx::raw_sql(m5).execute(&pool).await.expect("migration 5");

    let m6 = include_str!("../migrations/chat/0006_read_receipts.sql");
    sqlx::raw_sql(m6).execute(&pool).await.expect("migration 6");

    let m7 = include_str!("../migrations/chat/0007_reactions.sql");
    sqlx::raw_sql(m7).execute(&pool).await.expect("migration 7");

    let m8 = include_str!("../migrations/chat/0008_search.sql");
    sqlx::raw_sql(m8).execute(&pool).await.expect("migration 8");

    let m9 = include_str!("../migrations/chat/0009_enclaves.sql");
    sqlx::raw_sql(m9).execute(&pool).await.expect("migration 9");

    let m10 = include_str!("../migrations/chat/0010_room_name_per_enclave.sql");
    sqlx::raw_sql(m10)
        .execute(&pool)
        .await
        .expect("migration 10");

    pool
}

#[tokio::test]
async fn test_edit_message_updates_body_and_sets_edited_at() {
    let pool = setup_pool().await;

    let room_id = lets_chat::db::chat::create_room(&pool, "test-edit", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "original body")
        .await
        .unwrap();

    // Before edit: edited_at should be None
    let msg = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.body, "original body");
    assert!(msg.edited_at.is_none());

    // Edit the message
    let edited_at = lets_chat::db::chat::update_message_body(&pool, msg_id, "edited body")
        .await
        .unwrap();
    assert!(!edited_at.is_empty());

    // After edit: body and edited_at should be updated
    let msg = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.body, "edited body");
    assert_eq!(msg.edited_at, Some(edited_at));
}

#[tokio::test]
async fn test_get_message_returns_none_for_soft_deleted() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-softdelete", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    // Soft-delete the message
    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?",
    )
    .bind(msg_id)
    .execute(&pool)
    .await
    .unwrap();

    // get_message must return None for soft-deleted messages
    let result = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_list_messages_includes_edited_at() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-list-edited", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "first message")
        .await
        .unwrap();

    // Before edit: edited_at is None
    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].edited_at.is_none());
    assert_eq!(messages[0].body, "first message");

    // Edit it
    lets_chat::db::chat::update_message_body(&pool, msg_id, "updated message")
        .await
        .unwrap();

    // After edit: body updated, edited_at is Some
    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].edited_at.is_some());
    assert_eq!(messages[0].body, "updated message");
}

#[tokio::test]
async fn test_soft_deleted_messages_not_returned_by_list_messages() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-deleted-list", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "will be deleted")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "stays alive")
        .await
        .unwrap();

    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?",
    )
    .bind(msg_id)
    .execute(&pool)
    .await
    .unwrap();

    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, "stays alive");
}
