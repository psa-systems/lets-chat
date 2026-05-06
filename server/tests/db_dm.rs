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
    let auth_m3 = include_str!("../migrations/auth/0003_profile_fields.sql");
    sqlx::raw_sql(auth_m3)
        .execute(&auth_pool)
        .await
        .expect("auth migration 3");
    let auth_m4 = include_str!("../migrations/auth/0004_user_status.sql");
    sqlx::raw_sql(auth_m4)
        .execute(&auth_pool)
        .await
        .expect("auth migration 4");
    let auth_m5 = include_str!("../migrations/auth/0005_profile_visibility.sql");
    sqlx::raw_sql(auth_m5)
        .execute(&auth_pool)
        .await
        .expect("auth migration 5");

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
    let chat_m7 = include_str!("../migrations/chat/0007_reactions.sql");
    sqlx::raw_sql(chat_m7)
        .execute(&chat_pool)
        .await
        .expect("chat migration 7");
    let chat_m8 = include_str!("../migrations/chat/0008_search.sql");
    sqlx::raw_sql(chat_m8)
        .execute(&chat_pool)
        .await
        .expect("chat migration 8");
    let chat_m9 = include_str!("../migrations/chat/0009_enclaves.sql");
    sqlx::raw_sql(chat_m9)
        .execute(&chat_pool)
        .await
        .expect("chat migration 9");

    let chat_m10 = include_str!("../migrations/chat/0010_room_name_per_enclave.sql");
    sqlx::raw_sql(chat_m10)
        .execute(&chat_pool)
        .await
        .expect("chat migration 10");

    let chat_m11 = include_str!("../migrations/chat/0011_threads.sql");
    sqlx::raw_sql(chat_m11)
        .execute(&chat_pool)
        .await
        .expect("chat migration 11");

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
