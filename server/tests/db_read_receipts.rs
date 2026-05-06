use sqlx::SqlitePool;

async fn setup_chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
        include_str!("../migrations/chat/0009_enclaves.sql"),
        include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
        include_str!("../migrations/chat/0011_threads.sql"),
        include_str!("../migrations/chat/0014_mentions.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn upsert_is_monotonic() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    let m1 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "hi")
        .await
        .unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "again")
        .await
        .unwrap();

    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m2)
        .await
        .unwrap();
    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m1)
        .await
        .unwrap();

    let state = lets_chat::db::chat::get_dm_read_state(&pool, "user-a", room.id)
        .await
        .unwrap()
        .expect("state");
    assert_eq!(state.last_read_message_id, m2);
}

#[tokio::test]
async fn unread_counts_peer_only_above_watermark() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    let _m1 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "1")
        .await
        .unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "2")
        .await
        .unwrap();
    let _m3 = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "own")
        .await
        .unwrap();

    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a")
        .await
        .unwrap();
    let got = counts
        .iter()
        .find(|(r, _)| *r == room.id)
        .map(|(_, c)| *c)
        .unwrap_or(0);
    assert_eq!(got, 2);

    lets_chat::db::chat::upsert_dm_read(&pool, "user-a", room.id, m2)
        .await
        .unwrap();
    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a")
        .await
        .unwrap();
    let got = counts
        .iter()
        .find(|(r, _)| *r == room.id)
        .map(|(_, c)| *c)
        .unwrap_or(0);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn unread_counts_only_dms_user_is_in() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-b-c", "user-b", "user-c")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room.id, "user-b", "hi")
        .await
        .unwrap();

    let counts = lets_chat::db::chat::list_dm_unread_counts(&pool, "user-a")
        .await
        .unwrap();
    assert!(counts.iter().all(|(r, _)| *r != room.id));
}
