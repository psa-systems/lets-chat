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

    let chat_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    let chat_m1 = include_str!("../migrations/chat/0001_create_tables.sql");
    sqlx::raw_sql(chat_m1).execute(&chat_pool).await.expect("chat migration 1");
    let chat_m2 = include_str!("../migrations/chat/0002_moderation.sql");
    sqlx::raw_sql(chat_m2).execute(&chat_pool).await.expect("chat migration 2");
    let chat_m3 = include_str!("../migrations/chat/0003_dms.sql");
    sqlx::raw_sql(chat_m3).execute(&chat_pool).await.expect("chat migration 3");
    let chat_m4 = include_str!("../migrations/chat/0004_message_editing.sql");
    sqlx::raw_sql(chat_m4).execute(&chat_pool).await.expect("chat migration 4");
    let chat_m5 = include_str!("../migrations/chat/0005_private_rooms.sql");
    sqlx::raw_sql(chat_m5).execute(&chat_pool).await.expect("chat migration 5");

    (auth_pool, chat_pool)
}

#[tokio::test]
async fn test_list_rooms_excludes_dm_rooms() {
    let (_, chat_pool) = setup_pools().await;

    // Seeded rooms are public — admin sees all
    let rooms = lets_chat::db::chat::list_rooms(&chat_pool, "admin-user", true)
        .await
        .unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms.iter().all(|r| r.room_type == "public"));

    // Create a DM room
    lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();

    // list_rooms should still return only 2
    let rooms = lets_chat::db::chat::list_rooms(&chat_pool, "admin-user", true)
        .await
        .unwrap();
    assert_eq!(rooms.len(), 2);
}

#[tokio::test]
async fn test_create_dm_room_and_find() {
    let (_, chat_pool) = setup_pools().await;

    // No existing DM
    let found = lets_chat::db::chat::find_dm_room(&chat_pool, "user-a", "user-b")
        .await
        .unwrap();
    assert!(found.is_none());

    // Create DM
    let room = lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    assert_eq!(room.room_type, "dm");

    // Now find it
    let found = lets_chat::db::chat::find_dm_room(&chat_pool, "user-a", "user-b")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, room.id);

    // Find in reverse order too
    let found = lets_chat::db::chat::find_dm_room(&chat_pool, "user-b", "user-a")
        .await
        .unwrap();
    assert!(found.is_some());
}

#[tokio::test]
async fn test_list_user_dm_rooms() {
    let (_, chat_pool) = setup_pools().await;

    lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-c", "user-a", "user-c")
        .await
        .unwrap();

    let dms = lets_chat::db::chat::list_user_dm_rooms(&chat_pool, "user-a")
        .await
        .unwrap();
    assert_eq!(dms.len(), 2);

    // user-b should only see 1
    let dms = lets_chat::db::chat::list_user_dm_rooms(&chat_pool, "user-b")
        .await
        .unwrap();
    assert_eq!(dms.len(), 1);
    assert_eq!(dms[0].1, "user-a"); // other_user is user-a
}

#[tokio::test]
async fn test_dm_messages() {
    let (_, chat_pool) = setup_pools().await;

    let room = lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();

    lets_chat::db::chat::insert_message(&chat_pool, room.id, "user-a", "hello")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&chat_pool, room.id, "user-b", "hi back")
        .await
        .unwrap();

    let msgs = lets_chat::db::chat::list_messages(&chat_pool, room.id)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].body, "hello");
    assert_eq!(msgs[1].body, "hi back");
}
