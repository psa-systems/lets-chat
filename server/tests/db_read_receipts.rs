use sqlx::SqlitePool;

mod common;

async fn setup_chat_pool() -> SqlitePool {
    common::chat_pool().await
}

#[tokio::test]
async fn room_caught_up_member_ids_tracks_latest() {
    use lets_chat::db::chat;
    let pool = setup_chat_pool().await;
    let room = chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    for u in ["viewer", "alice", "bob"] {
        chat::add_room_member(&pool, room, u).await.unwrap();
    }
    let m1 = chat::insert_message(&pool, room, "viewer", "one")
        .await
        .unwrap();
    let m2 = chat::insert_message(&pool, room, "viewer", "two")
        .await
        .unwrap();

    // alice read everything; bob only the first message.
    chat::set_last_read(&pool, "alice", room, m2).await.unwrap();
    chat::set_last_read(&pool, "bob", room, m1).await.unwrap();

    let caught = chat::room_caught_up_member_ids(&pool, room, "viewer")
        .await
        .unwrap();
    assert_eq!(caught, vec!["alice".to_string()], "only alice reached m2");

    // bob catches up -> both, viewer always excluded.
    chat::set_last_read(&pool, "bob", room, m2).await.unwrap();
    let mut caught = chat::room_caught_up_member_ids(&pool, room, "viewer")
        .await
        .unwrap();
    caught.sort();
    assert_eq!(caught, vec!["alice".to_string(), "bob".to_string()]);
    assert!(!caught.contains(&"viewer".to_string()));
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
