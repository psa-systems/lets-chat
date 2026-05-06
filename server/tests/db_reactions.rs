use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    for (i, sql) in [
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
    ]
    .iter()
    .enumerate()
    {
        sqlx::raw_sql(sql)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("migration {} failed: {e}", i + 1));
    }

    pool
}

#[tokio::test]
async fn test_add_reaction_returns_true() {
    let pool = setup_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "test-react", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    let added = lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "👍")
        .await
        .unwrap();
    assert!(added, "first toggle should add the reaction");
}

#[tokio::test]
async fn test_remove_reaction_returns_false() {
    let pool = setup_pool().await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-react-rm", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    // Add then remove
    lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "👍")
        .await
        .unwrap();
    let removed = lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "👍")
        .await
        .unwrap();
    assert!(!removed, "second toggle should remove the reaction");
}

#[tokio::test]
async fn test_list_reactions_aggregates_counts() {
    let pool = setup_pool().await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-react-list", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hi")
        .await
        .unwrap();

    // Three users react with 👍; two also react with ❤️
    for uid in ["user-1", "user-2", "user-3"] {
        lets_chat::db::chat::toggle_reaction(&pool, msg_id, uid, "👍")
            .await
            .unwrap();
    }
    for uid in ["user-1", "user-2"] {
        lets_chat::db::chat::toggle_reaction(&pool, msg_id, uid, "❤️")
            .await
            .unwrap();
    }

    let reactions = lets_chat::db::chat::list_reactions(&pool, msg_id, "user-1")
        .await
        .unwrap();

    let thumbs = reactions.iter().find(|r| r.emoji == "👍").unwrap();
    assert_eq!(thumbs.count, 3);
    assert!(thumbs.reacted_by_me, "user-1 reacted with 👍");

    let heart = reactions.iter().find(|r| r.emoji == "❤️").unwrap();
    assert_eq!(heart.count, 2);
    assert!(heart.reacted_by_me, "user-1 reacted with ❤️");
}

#[tokio::test]
async fn test_list_reactions_reacted_by_me_false_for_other_user() {
    let pool = setup_pool().await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-react-other", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    // Only user-1 reacts
    lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "🔥")
        .await
        .unwrap();

    // List from user-2's perspective
    let reactions = lets_chat::db::chat::list_reactions(&pool, msg_id, "user-2")
        .await
        .unwrap();

    let fire = reactions.iter().find(|r| r.emoji == "🔥").unwrap();
    assert_eq!(fire.count, 1);
    assert!(!fire.reacted_by_me, "user-2 did not react");
}

#[tokio::test]
async fn test_reaction_removed_disappears_from_list() {
    let pool = setup_pool().await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-react-gone", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "😂")
        .await
        .unwrap();
    lets_chat::db::chat::toggle_reaction(&pool, msg_id, "user-1", "😂")
        .await
        .unwrap();

    let reactions = lets_chat::db::chat::list_reactions(&pool, msg_id, "user-1")
        .await
        .unwrap();
    assert!(
        !reactions.iter().any(|r| r.emoji == "😂"),
        "emoji should be gone after toggle-off"
    );
}
