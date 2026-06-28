use sqlx::SqlitePool;

mod common;

async fn setup_pool() -> SqlitePool {
    common::chat_pool().await
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

#[tokio::test]
async fn test_top_reaction_emojis_ranks_by_frequency_unicode_only() {
    let pool = setup_pool().await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-react-top", None, "public", None, None)
            .await
            .unwrap();
    // Three messages so user-1 can react with the same emoji repeatedly
    // (one reaction row per (message, user, emoji)).
    let mut msgs = Vec::new();
    for i in 0..3 {
        msgs.push(
            lets_chat::db::chat::insert_message(&pool, room_id, "user-1", &format!("m{i}"))
                .await
                .unwrap(),
        );
    }

    // 👍 x3, ❤️ x2, 😂 x1 for user-1, plus a custom :party: that must be excluded.
    for m in &msgs {
        lets_chat::db::chat::toggle_reaction(&pool, *m, "user-1", "👍")
            .await
            .unwrap();
    }
    for m in &msgs[..2] {
        lets_chat::db::chat::toggle_reaction(&pool, *m, "user-1", "❤️")
            .await
            .unwrap();
    }
    lets_chat::db::chat::toggle_reaction(&pool, msgs[0], "user-1", "😂")
        .await
        .unwrap();
    lets_chat::db::chat::toggle_reaction(&pool, msgs[0], "user-1", ":party:")
        .await
        .unwrap();
    // Another user's reactions must not leak into user-1's ranking.
    lets_chat::db::chat::toggle_reaction(&pool, msgs[0], "user-2", "😡")
        .await
        .unwrap();

    let top = lets_chat::db::chat::top_reaction_emojis(&pool, "user-1", 8)
        .await
        .unwrap();

    assert_eq!(
        top,
        vec!["👍".to_string(), "❤️".to_string(), "😂".to_string()],
        "frequency-ranked, custom :shortcode: excluded, other users excluded"
    );

    // limit caps the row count
    let top2 = lets_chat::db::chat::top_reaction_emojis(&pool, "user-1", 2)
        .await
        .unwrap();
    assert_eq!(top2, vec!["👍".to_string(), "❤️".to_string()]);
}
