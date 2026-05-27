use lets_chat::db::bookmarks::{
    bookmark_message, bookmarked_message_ids_in_room, bookmarks_for_user, is_bookmarked,
    room_for_message, unbookmark_message,
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
        include_str!("../migrations/chat/0033_scheduled_messages.sql"),
        include_str!("../migrations/chat/0034_branding.sql"),
        include_str!("../migrations/chat/0035_analytics_daily.sql"),
        include_str!("../migrations/chat/0036_branding_favicon.sql"),
        include_str!("../migrations/chat/0037_reminders.sql"),
        include_str!("../migrations/chat/0038_polls.sql"),
        include_str!("../migrations/chat/0039_slash_commands_custom.sql"),
        include_str!("../migrations/chat/0040_enclave_last_room.sql"),
        include_str!("../migrations/chat/0041_incoming_webhooks.sql"),
        include_str!("../migrations/chat/0042_outgoing_webhooks.sql"),
        include_str!("../migrations/chat/0043_room_retention.sql"),
        include_str!("../migrations/chat/0044_link_filter_quarantine_cascade.sql"),
        include_str!("../migrations/chat/0045_messages_fts_delete_trigger.sql"),
        include_str!("../migrations/chat/0046_messages_fts_purge_guard.sql"),
        include_str!("../migrations/chat/0047_message_drafts.sql"),
        include_str!("../migrations/chat/0048_email_inboxes.sql"),
        include_str!("../migrations/chat/0049_messages_email_inbox_id.sql"),
        include_str!("../migrations/chat/0050_reply_tokens.sql"),
        include_str!("../migrations/chat/0051_processed_message_ids.sql"),
        include_str!("../migrations/chat/0052_remote_control_sessions.sql"),
        include_str!("../migrations/chat/0053_room_feeds.sql"),
        include_str!("../migrations/chat/0054_bridges.sql"),
        include_str!("../migrations/chat/0055_messages_bridge_actor.sql"),
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
async fn bookmark_then_unbookmark_round_trips() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    assert!(is_bookmarked(&pool, "viewer", msg).await.unwrap());

    unbookmark_message(&pool, "viewer", msg).await.unwrap();
    assert!(!is_bookmarked(&pool, "viewer", msg).await.unwrap());
}

#[tokio::test]
async fn bookmark_is_idempotent() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    bookmark_message(&pool, "viewer", msg).await.unwrap();

    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn unbookmark_nonexistent_is_a_noop() {
    let pool = setup_chat_pool().await;
    unbookmark_message(&pool, "viewer", 99999).await.unwrap();
}

#[tokio::test]
async fn bookmarks_are_private_per_user() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "alice", msg).await.unwrap();
    assert!(is_bookmarked(&pool, "alice", msg).await.unwrap());
    assert!(!is_bookmarked(&pool, "bob", msg).await.unwrap());

    let alice_rows = bookmarks_for_user(&pool, "alice").await.unwrap();
    assert_eq!(alice_rows.len(), 1);
    let bob_rows = bookmarks_for_user(&pool, "bob").await.unwrap();
    assert!(bob_rows.is_empty());
}

#[tokio::test]
async fn bookmarks_for_user_excludes_soft_deleted_messages() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let kept = seed_message(&pool, room, "author", "kept").await;
    let gone = seed_message(&pool, room, "author", "gone").await;

    bookmark_message(&pool, "viewer", kept).await.unwrap();
    bookmark_message(&pool, "viewer", gone).await.unwrap();

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, kept);
}

#[tokio::test]
async fn bookmarked_message_ids_in_room_is_room_scoped() {
    let pool = setup_chat_pool().await;
    let room_a = seed_room(&pool, "alpha", "public").await;
    let room_b = seed_room(&pool, "beta", "public").await;
    let in_a = seed_message(&pool, room_a, "author", "a").await;
    let in_b = seed_message(&pool, room_b, "author", "b").await;

    bookmark_message(&pool, "viewer", in_a).await.unwrap();
    bookmark_message(&pool, "viewer", in_b).await.unwrap();

    let a_ids = bookmarked_message_ids_in_room(&pool, "viewer", room_a)
        .await
        .unwrap();
    assert!(a_ids.contains(&in_a));
    assert!(!a_ids.contains(&in_b));
}

#[tokio::test]
async fn cascading_delete_of_message_removes_bookmark_row() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(msg)
        .execute(&pool)
        .await
        .unwrap();

    assert!(!is_bookmarked(&pool, "viewer", msg).await.unwrap());
}

#[tokio::test]
async fn room_for_message_returns_none_for_soft_deleted() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    assert_eq!(room_for_message(&pool, msg).await.unwrap(), Some(room));

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(msg)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(room_for_message(&pool, msg).await.unwrap(), None);
}
