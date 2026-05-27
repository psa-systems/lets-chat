use lets_chat::db::notifications::{
    self, room_mute_mode, room_mute_modes_for_user, set_room_mute_mode, MuteMode,
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
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
        include_str!("../migrations/chat/0016_pinned_messages.sql"),
        include_str!("../migrations/chat/0017_custom_emojis.sql"),
        include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
        include_str!("../migrations/chat/0019_bookmarks.sql"),
        include_str!("../migrations/chat/0032_anti_spam.sql"),
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

async fn seed_room(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'public')")
        .bind(name)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn lookup_returns_none_when_no_row() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn set_and_lookup_round_trip() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
}

#[tokio::test]
async fn upsert_overwrites_existing_mode() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
}

#[tokio::test]
async fn setting_to_none_deletes_row() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    set_room_mute_mode(&pool, "user-1", r, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::None)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind("user-1")
    .bind(r)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn delete_helper_is_idempotent() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    notifications::delete_room_mute_setting(&pool, "user-1", r)
        .await
        .unwrap();
    notifications::delete_room_mute_setting(&pool, "user-1", r)
        .await
        .unwrap();
}

#[tokio::test]
async fn bulk_loader_returns_only_set_rooms() {
    let pool = setup_chat_pool().await;
    let r1 = seed_room(&pool, "alpha").await;
    let r2 = seed_room(&pool, "beta").await;
    let _r3 = seed_room(&pool, "gamma").await;
    set_room_mute_mode(&pool, "user-1", r1, MuteMode::All)
        .await
        .unwrap();
    set_room_mute_mode(&pool, "user-1", r2, MuteMode::ExceptMentions)
        .await
        .unwrap();
    let map = room_mute_modes_for_user(&pool, "user-1").await.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&r1), Some(&MuteMode::All));
    assert_eq!(map.get(&r2), Some(&MuteMode::ExceptMentions));
}

#[tokio::test]
async fn check_constraint_rejects_unknown_mode() {
    let pool = setup_chat_pool().await;
    let r = seed_room(&pool, "general").await;
    let res = sqlx::query(
        "INSERT INTO room_notification_settings (user_id, room_id, mute_mode) \
         VALUES (?, ?, ?)",
    )
    .bind("user-1")
    .bind(r)
    .bind("bogus")
    .execute(&pool)
    .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn parse_str_known_values_round_trip() {
    for m in [MuteMode::None, MuteMode::ExceptMentions, MuteMode::All] {
        assert_eq!(MuteMode::parse_str(m.as_str()), Some(m));
    }
    assert!(MuteMode::parse_str("nope").is_none());
}

#[test]
fn allows_room_mention_only_blocks_in_all_mode() {
    assert!(MuteMode::None.allows_room_mention());
    assert!(MuteMode::ExceptMentions.allows_room_mention());
    assert!(!MuteMode::All.allows_room_mention());
}

#[test]
fn allows_unread_bump_only_in_none_mode() {
    assert!(MuteMode::None.allows_unread_bump());
    assert!(!MuteMode::ExceptMentions.allows_unread_bump());
    assert!(!MuteMode::All.allows_unread_bump());
}
