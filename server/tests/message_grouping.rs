use lets_chat::db;
use lets_chat::db::chat::{is_follow_up_of, MESSAGE_GROUPING_WINDOW_SECONDS};
use sqlx::SqlitePool;

async fn setup_pools() -> (SqlitePool, SqlitePool) {
    let auth_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&auth_pool).await.unwrap();
    }

    let chat_pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
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
        sqlx::raw_sql(sql).execute(&chat_pool).await.unwrap();
    }

    (auth_pool, chat_pool)
}

#[test]
fn follow_up_when_same_user_within_window() {
    assert!(is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:00:30"),
    ));
}

#[test]
fn not_follow_up_when_different_user() {
    assert!(!is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("bob", "2026-05-04 12:00:30"),
    ));
}

#[test]
fn not_follow_up_when_gap_exceeds_window() {
    assert!(!is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:06:00"),
    ));
}

#[test]
fn follow_up_at_exact_window_boundary() {
    assert!(is_follow_up_of(
        Some(("alice", "2026-05-04 12:00:00")),
        ("alice", "2026-05-04 12:05:00"),
    ));
}

#[test]
fn not_follow_up_when_no_prior() {
    assert!(!is_follow_up_of(None, ("alice", "2026-05-04 12:00:00"),));
}

#[test]
fn window_is_five_minutes() {
    assert_eq!(MESSAGE_GROUPING_WINDOW_SECONDS, 300);
}

#[tokio::test]
async fn loader_marks_consecutive_same_user_as_follow_ups() {
    let (auth_pool, chat_pool) = setup_pools().await;

    let user_id = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    let _ = db::chat::insert_message(&chat_pool, room_id, &user_id, "first")
        .await
        .unwrap();
    let _ = db::chat::insert_message(&chat_pool, room_id, &user_id, "second")
        .await
        .unwrap();
    let _ = db::chat::insert_message(&chat_pool, room_id, &user_id, "third")
        .await
        .unwrap();

    let raw = db::chat::list_messages(&chat_pool, room_id).await.unwrap();
    assert_eq!(raw.len(), 3);
    let mut prev: Option<(String, String)> = None;
    let mut flags: Vec<bool> = Vec::new();
    for m in &raw {
        let is_fu = is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        );
        prev = Some((m.user_id.clone(), m.created_at.clone()));
        flags.push(is_fu);
    }
    assert_eq!(flags, vec![false, true, true]);
}

#[tokio::test]
async fn loader_breaks_grouping_on_different_user() {
    let (auth_pool, chat_pool) = setup_pools().await;
    let alice = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let bob = db::auth::create_user(&auth_pool, "bob", "x").await.unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    db::chat::insert_message(&chat_pool, room_id, &alice, "a1")
        .await
        .unwrap();
    db::chat::insert_message(&chat_pool, room_id, &bob, "b1")
        .await
        .unwrap();
    db::chat::insert_message(&chat_pool, room_id, &alice, "a2")
        .await
        .unwrap();

    let raw = db::chat::list_messages(&chat_pool, room_id).await.unwrap();
    let mut prev: Option<(String, String)> = None;
    let mut flags: Vec<bool> = Vec::new();
    for m in &raw {
        flags.push(is_follow_up_of(
            prev.as_ref().map(|(u, t)| (u.as_str(), t.as_str())),
            (&m.user_id, &m.created_at),
        ));
        prev = Some((m.user_id.clone(), m.created_at.clone()));
    }
    assert_eq!(flags, vec![false, false, false]);
}

#[tokio::test]
async fn prior_message_in_room_returns_immediately_prior() {
    let (auth_pool, chat_pool) = setup_pools().await;
    let user_id = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    let id1 = db::chat::insert_message(&chat_pool, room_id, &user_id, "first")
        .await
        .unwrap();
    let id2 = db::chat::insert_message(&chat_pool, room_id, &user_id, "second")
        .await
        .unwrap();

    let prior_of_2 = db::chat::prior_message_in_room(&chat_pool, room_id, id2)
        .await
        .unwrap();
    assert_eq!(prior_of_2.unwrap().id, id1);

    let prior_of_1 = db::chat::prior_message_in_room(&chat_pool, room_id, id1)
        .await
        .unwrap();
    assert!(prior_of_1.is_none());
}

#[tokio::test]
async fn delete_header_marks_next_for_promotion() {
    let (auth_pool, chat_pool) = setup_pools().await;
    let user_id = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    let id1 = db::chat::insert_message(&chat_pool, room_id, &user_id, "first")
        .await
        .unwrap();
    let _id2 = db::chat::insert_message(&chat_pool, room_id, &user_id, "second")
        .await
        .unwrap();
    let _id3 = db::chat::insert_message(&chat_pool, room_id, &user_id, "third")
        .await
        .unwrap();

    let target = db::chat::get_message(&chat_pool, id1)
        .await
        .unwrap()
        .unwrap();
    let next = db::chat::next_message_in_room(&chat_pool, room_id, id1)
        .await
        .unwrap()
        .unwrap();

    let was_follow_up = is_follow_up_of(
        Some((target.user_id.as_str(), target.created_at.as_str())),
        (next.user_id.as_str(), next.created_at.as_str()),
    );
    assert!(was_follow_up, "next was a follow-up of the deleted header");

    db::moderation::soft_delete_message(&chat_pool, id1, &user_id)
        .await
        .unwrap();

    let new_prior = db::chat::prior_message_in_room(&chat_pool, room_id, next.id)
        .await
        .unwrap();
    let promoted_flag = is_follow_up_of(
        new_prior
            .as_ref()
            .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
        (next.user_id.as_str(), next.created_at.as_str()),
    );
    assert!(
        !promoted_flag,
        "after delete the next message must render as a header"
    );
}

#[tokio::test]
async fn delete_lone_message_no_promote() {
    let (auth_pool, chat_pool) = setup_pools().await;
    let user_id = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    let only = db::chat::insert_message(&chat_pool, room_id, &user_id, "only")
        .await
        .unwrap();
    let next = db::chat::next_message_in_room(&chat_pool, room_id, only)
        .await
        .unwrap();
    assert!(next.is_none(), "no next message in single-message thread");

    db::moderation::soft_delete_message(&chat_pool, only, &user_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn next_message_in_room_returns_immediately_next() {
    let (auth_pool, chat_pool) = setup_pools().await;
    let user_id = db::auth::create_user(&auth_pool, "alice", "x")
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "grouping-test", None, "public", None, None)
        .await
        .unwrap();

    let id1 = db::chat::insert_message(&chat_pool, room_id, &user_id, "first")
        .await
        .unwrap();
    let id2 = db::chat::insert_message(&chat_pool, room_id, &user_id, "second")
        .await
        .unwrap();

    let next_of_1 = db::chat::next_message_in_room(&chat_pool, room_id, id1)
        .await
        .unwrap();
    assert_eq!(next_of_1.unwrap().id, id2);

    let next_of_2 = db::chat::next_message_in_room(&chat_pool, room_id, id2)
        .await
        .unwrap();
    assert!(next_of_2.is_none());
}
