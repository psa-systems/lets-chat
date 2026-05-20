use lets_chat::db::pinned::{
    self, count_for_room, pin_message, pinned_message_ids_for_room, pins_for_room, unpin_message,
};
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
        include_str!("../migrations/chat/0012_uploads.sql"),
        include_str!("../migrations/chat/0013_link_previews.sql"),
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
        include_str!("../migrations/chat/0016_pinned_messages.sql"),
        include_str!("../migrations/chat/0017_custom_emojis.sql"),
        include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
        include_str!("../migrations/chat/0019_bookmarks.sql"),
        include_str!("../migrations/chat/0032_anti_spam.sql"),
        include_str!("../migrations/chat/0033_branding.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn seed_room(pool: &SqlitePool, name: &str, kind: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, ?)")
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn seed_message(pool: &SqlitePool, room_id: i64, user_id: &str, body: &str) -> i64 {
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn pin_then_unpin_round_trips() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "u1", "hello").await;

    pin_message(&pool, msg, room, "u2").await.unwrap();
    assert_eq!(count_for_room(&pool, room).await.unwrap(), 1);

    unpin_message(&pool, msg).await.unwrap();
    assert_eq!(count_for_room(&pool, room).await.unwrap(), 0);
}

#[tokio::test]
async fn pin_idempotent_when_already_pinned() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "u1", "hello").await;

    pin_message(&pool, msg, room, "u2").await.unwrap();
    pin_message(&pool, msg, room, "u3").await.unwrap();
    assert_eq!(count_for_room(&pool, room).await.unwrap(), 1);

    // Original pinner is preserved on the no-op second call.
    let pins = pins_for_room(&pool, room, 10).await.unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].pinned_by_user_id, "u2");
}

#[tokio::test]
async fn unpin_nonexistent_is_a_noop() {
    let pool = setup_chat_pool().await;
    unpin_message(&pool, 99999).await.unwrap();
}

#[tokio::test]
async fn pins_for_room_excludes_soft_deleted_messages() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let kept = seed_message(&pool, room, "u1", "kept").await;
    let gone = seed_message(&pool, room, "u1", "deleted").await;

    pin_message(&pool, kept, room, "u2").await.unwrap();
    pin_message(&pool, gone, room, "u2").await.unwrap();

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    // Both pin rows still exist in the table
    let raw_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(raw_count, 2);

    // ...but the visible count and list filter the deleted one out.
    assert_eq!(count_for_room(&pool, room).await.unwrap(), 1);
    let pins = pins_for_room(&pool, room, 10).await.unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].message_id, kept);

    let ids = pinned_message_ids_for_room(&pool, room).await.unwrap();
    assert!(ids.contains(&kept));
    assert!(!ids.contains(&gone));
}

#[tokio::test]
async fn pin_cap_returns_protocol_error_on_overage() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    for i in 0..pinned::MAX_PINS_PER_ROOM {
        let mid = seed_message(&pool, room, "u1", &format!("m{i}")).await;
        pin_message(&pool, mid, room, "u2").await.unwrap();
    }
    let extra = seed_message(&pool, room, "u1", "overflow").await;
    let err = pin_message(&pool, extra, room, "u2").await.unwrap_err();
    match err {
        sqlx::Error::Protocol(s) => assert!(
            s.contains("pin cap reached"),
            "unexpected protocol error body: {s}"
        ),
        other => panic!("expected Protocol error, got {other:?}"),
    }
    assert_eq!(
        count_for_room(&pool, room).await.unwrap(),
        pinned::MAX_PINS_PER_ROOM
    );
}

#[tokio::test]
async fn cascade_delete_on_message_hard_delete() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "u1", "hello").await;
    pin_message(&pool, msg, room, "u2").await.unwrap();

    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(msg)
        .execute(&pool)
        .await
        .unwrap();

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE message_id = ?")
        .bind(msg)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn cascade_delete_on_room_delete() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "u1", "hello").await;
    pin_message(&pool, msg, room, "u2").await.unwrap();

    // Removing the room should cascade through messages -> pinned_messages.
    sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(room)
        .execute(&pool)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn pinned_message_ids_for_room_empty_when_nothing_pinned() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let _ = seed_message(&pool, room, "u1", "hello").await;
    let ids = pinned_message_ids_for_room(&pool, room).await.unwrap();
    assert!(ids.is_empty());
}
