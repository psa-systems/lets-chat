use sqlx::SqlitePool;

mod common;

async fn setup_chat_pool() -> SqlitePool {
    common::chat_pool().await
}

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
        include_str!("../migrations/auth/0008_two_factor.sql"),
        include_str!("../migrations/auth/0009_push_subscriptions.sql"),
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
        include_str!("../migrations/auth/0012_session_metadata.sql"),
        include_str!("../migrations/auth/0013_digest_columns.sql"),
        include_str!("../migrations/auth/0014_login_alerts.sql"),
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sidebar_categories.sql"),
        include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql"),
        include_str!("../migrations/auth/0018_starred_rooms.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn parse_extracts_distinct_tokens() {
    let tokens =
        lets_chat::db::mentions::parse_mention_tokens("hi @alice, please cc @bob and @alice again");
    assert_eq!(tokens, vec!["alice".to_string(), "bob".to_string()]);
}

#[tokio::test]
async fn parse_ignores_email_addresses() {
    // The token regex requires `(^|\s)` before `@`, so `foo@bar.com` does NOT
    // yield a `bar` mention. Critical for not converting email addresses
    // into stray pings.
    let tokens = lets_chat::db::mentions::parse_mention_tokens("ping foo@bar.com");
    assert_eq!(tokens, Vec::<String>::new());
}

#[tokio::test]
async fn parse_matches_at_start_of_string() {
    let tokens = lets_chat::db::mentions::parse_mention_tokens("@alice hi");
    assert_eq!(tokens, vec!["alice".to_string()]);
}

#[tokio::test]
async fn reconcile_inserts_added_and_removes_dropped() {
    let pool = setup_chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "author", "hi @alice")
        .await
        .unwrap();

    let alice = lets_chat::db::mentions::MentionRef {
        user_id: "alice-id".into(),
        username: "alice".into(),
    };
    let bob = lets_chat::db::mentions::MentionRef {
        user_id: "bob-id".into(),
        username: "bob".into(),
    };

    // Initial set: {alice}
    let (added, removed) = lets_chat::db::mentions::reconcile_mentions(
        &pool,
        msg_id,
        room_id,
        "author",
        std::slice::from_ref(&alice),
    )
    .await
    .unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(removed.len(), 0);

    // Replace with {bob}: alice removed, bob added.
    let (added2, removed2) = lets_chat::db::mentions::reconcile_mentions(
        &pool,
        msg_id,
        room_id,
        "author",
        std::slice::from_ref(&bob),
    )
    .await
    .unwrap();
    assert_eq!(added2.len(), 1);
    assert_eq!(added2[0].user_id, "bob-id");
    assert_eq!(removed2.len(), 1);
    assert_eq!(removed2[0].user_id, "alice-id");

    // Idempotent reconcile with the same {bob}: no change.
    let (added3, removed3) =
        lets_chat::db::mentions::reconcile_mentions(&pool, msg_id, room_id, "author", &[bob])
            .await
            .unwrap();
    assert_eq!(added3.len(), 0);
    assert_eq!(removed3.len(), 0);
}

#[tokio::test]
async fn watermark_advances_read_state() {
    let pool = setup_chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    let m1 = lets_chat::db::chat::insert_message(&pool, room_id, "author", "@alice 1")
        .await
        .unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room_id, "author", "@alice 2")
        .await
        .unwrap();
    let alice = lets_chat::db::mentions::MentionRef {
        user_id: "alice-id".into(),
        username: "alice".into(),
    };
    lets_chat::db::mentions::reconcile_mentions(
        &pool,
        m1,
        room_id,
        "author",
        std::slice::from_ref(&alice),
    )
    .await
    .unwrap();
    lets_chat::db::mentions::reconcile_mentions(&pool, m2, room_id, "author", &[alice])
        .await
        .unwrap();

    // Both unread.
    let counts = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, "alice-id")
        .await
        .unwrap();
    assert_eq!(
        counts.iter().find(|(r, _)| *r == room_id).map(|(_, n)| *n),
        Some(2)
    );

    // Advance watermark past m1 only.
    let flipped =
        lets_chat::db::mentions::mark_mentions_read_for_room(&pool, "alice-id", room_id, m1)
            .await
            .unwrap();
    assert_eq!(flipped, 1);
    let counts = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, "alice-id")
        .await
        .unwrap();
    assert_eq!(
        counts.iter().find(|(r, _)| *r == room_id).map(|(_, n)| *n),
        Some(1)
    );

    // Advance past m2.
    lets_chat::db::mentions::mark_mentions_read_for_room(&pool, "alice-id", room_id, m2)
        .await
        .unwrap();
    let counts = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, "alice-id")
        .await
        .unwrap();
    assert!(!counts.iter().any(|(r, _)| *r == room_id));
}

#[tokio::test]
async fn mentions_for_messages_resolves_usernames() {
    let chat = setup_chat_pool().await;
    let auth = setup_auth_pool().await;

    let alice_id = lets_chat::db::auth::create_user(&auth, "alice", "h")
        .await
        .unwrap();
    let _bob_id = lets_chat::db::auth::create_user(&auth, "bob", "h")
        .await
        .unwrap();

    let room_id = lets_chat::db::chat::create_room(&chat, "general", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&chat, room_id, "author", "hi @alice")
        .await
        .unwrap();

    let alice = lets_chat::db::mentions::MentionRef {
        user_id: alice_id.clone(),
        username: "alice".into(),
    };
    lets_chat::db::mentions::reconcile_mentions(&chat, msg_id, room_id, "author", &[alice])
        .await
        .unwrap();

    let by_message = lets_chat::db::mentions::mentions_for_messages(&chat, &auth, &[msg_id])
        .await
        .unwrap();
    let list = by_message.get(&msg_id).expect("message present");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].user_id, alice_id);
    assert_eq!(list[0].username, "alice");
}

#[tokio::test]
async fn delete_mentions_returns_unread_users() {
    let pool = setup_chat_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "author", "@alice @bob")
        .await
        .unwrap();
    let alice = lets_chat::db::mentions::MentionRef {
        user_id: "alice-id".into(),
        username: "alice".into(),
    };
    let bob = lets_chat::db::mentions::MentionRef {
        user_id: "bob-id".into(),
        username: "bob".into(),
    };
    lets_chat::db::mentions::reconcile_mentions(&pool, msg_id, room_id, "author", &[alice, bob])
        .await
        .unwrap();
    // Mark alice read.
    lets_chat::db::mentions::mark_mentions_read_for_room(&pool, "alice-id", room_id, msg_id)
        .await
        .unwrap();

    // Delete all rows for the message; only bob still had an unread row.
    let users = lets_chat::db::mentions::delete_mentions_for_message(&pool, msg_id)
        .await
        .unwrap();
    assert_eq!(users, vec!["bob-id".to_string()]);

    // Both alice and bob should now have zero unread (rows are gone).
    let counts_a = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, "alice-id")
        .await
        .unwrap();
    let counts_b = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, "bob-id")
        .await
        .unwrap();
    assert!(counts_a.is_empty());
    assert!(counts_b.is_empty());
}
