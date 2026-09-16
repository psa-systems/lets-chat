use sqlx::SqlitePool;

// Run the full embedded migration set rather than a hand-listed subset:
// `sqlx::migrate!` picks up every file in the migrations dir at compile
// time, so new migrations land in tests automatically instead of silently
// going stale (which left `messages` without the `quote_id` column here).
async fn setup_pools() -> (SqlitePool, SqlitePool) {
    let auth_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    sqlx::migrate!("./migrations/auth")
        .run(&auth_pool)
        .await
        .expect("auth migrations");

    let chat_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    sqlx::migrate!("./migrations/chat")
        .run(&chat_pool)
        .await
        .expect("chat migrations");

    (auth_pool, chat_pool)
}

#[tokio::test]
async fn test_list_rooms_excludes_dm_rooms() {
    let (_, chat_pool) = setup_pools().await;

    // Seeded rooms are public - admin sees all
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

// LC-909: `create_dm_room`'s second, racing insert must fail the `dm_pairs`
// UNIQUE constraint and hand back the winner's room instead of erroring or
// leaving a second `dm` room behind for the pair.
#[tokio::test]
async fn test_create_dm_room_loses_race_returns_existing_room() {
    let (_, chat_pool) = setup_pools().await;

    let first = lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();

    // A second creator for the same pair (e.g. a retry, or a second racing
    // caller that also lost its find_dm_room check) must resolve to the
    // existing room, not create a duplicate.
    let second =
        lets_chat::db::chat::create_dm_room(&chat_pool, "dm-a-b-again", "user-b", "user-a")
            .await
            .unwrap();
    assert_eq!(second.id, first.id);

    let dm_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rooms WHERE room_type = 'dm'")
        .fetch_one(&chat_pool)
        .await
        .unwrap();
    assert_eq!(dm_count, 1, "only one dm room must exist for the pair");
}

// LC-909: the exact race the assistant-bot support bubble hits - a WS-driven
// re-render and an in-flight HTTP request both resolving `support_dm_room`
// for a user with no bot DM yet. Both concurrent `find_or_create_dm_room`
// calls must return the same room, and exactly one `dm` room must exist
// joining the two users afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_find_or_create_dm_room_yields_one_room() {
    let (_, chat_pool) = setup_pools().await;

    let pool_a = chat_pool.clone();
    let pool_b = chat_pool.clone();
    let (res_a, res_b) = tokio::join!(
        tokio::spawn(async move {
            lets_chat::db::chat::find_or_create_dm_room(&pool_a, "@bot", "bot", "carol").await
        }),
        tokio::spawn(async move {
            lets_chat::db::chat::find_or_create_dm_room(&pool_b, "@bot", "bot", "carol").await
        }),
    );
    let (room_a, _created_a) = res_a.expect("join a").expect("call a");
    let (room_b, _created_b) = res_b.expect("join b").expect("call b");

    assert_eq!(
        room_a.id, room_b.id,
        "both concurrent calls must resolve to the same room"
    );

    let dm_rooms: Vec<i64> = sqlx::query_scalar(
        "SELECT r.id FROM rooms r \
         JOIN room_members m1 ON m1.room_id = r.id AND m1.user_id = 'bot' \
         JOIN room_members m2 ON m2.room_id = r.id AND m2.user_id = 'carol' \
         WHERE r.room_type = 'dm'",
    )
    .fetch_all(&chat_pool)
    .await
    .unwrap();
    assert_eq!(
        dm_rooms.len(),
        1,
        "exactly one dm room must join bot and carol, got: {dm_rooms:?}"
    );
}
