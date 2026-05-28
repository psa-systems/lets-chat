use lets_chat::db::notifications::{
    self, room_mute_mode, set_dm_mute, set_room_mute_mode, MuteMode,
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
        include_str!("../migrations/chat/0056_bridge_avatar_proxies.sql"),
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

#[tokio::test]
async fn set_dm_mute_writes_all_mode() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_false_deletes_row() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, false).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind("user-1")
    .bind(dm)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn set_dm_mute_idempotent_when_already_muted() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
}

#[tokio::test]
async fn set_dm_mute_rejects_non_dm_room() {
    let pool = setup_chat_pool().await;
    let public = seed_room(&pool, "general", "public").await;
    let res = set_dm_mute(&pool, "user-1", public, true).await;
    assert!(res.is_err(), "expected error, got {res:?}");
}

#[tokio::test]
async fn set_dm_mute_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_dm_mute(&pool, "user-1", 9_999, true).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_dm_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::All).await;
    assert!(res.is_err(), "expected DM rejection, got {res:?}");
    let res = set_room_mute_mode(&pool, "user-1", dm, MuteMode::ExceptMentions).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn set_room_mute_mode_rejects_missing_room() {
    let pool = setup_chat_pool().await;
    let res = set_room_mute_mode(&pool, "user-1", 9_999, MuteMode::All).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn dm_mute_per_direction_independent() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    set_dm_mute(&pool, "user-a", dm, true).await.unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-a", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-b", dm).await.unwrap(),
        MuteMode::None
    );
}

#[tokio::test]
async fn dm_mute_does_not_collide_with_room_mute_for_other_room() {
    let pool = setup_chat_pool().await;
    let dm = seed_room(&pool, "@bob", "dm").await;
    let r = seed_room(&pool, "general", "public").await;
    set_dm_mute(&pool, "user-1", dm, true).await.unwrap();
    set_room_mute_mode(&pool, "user-1", r, MuteMode::ExceptMentions)
        .await
        .unwrap();
    assert_eq!(
        room_mute_mode(&pool, "user-1", dm).await.unwrap(),
        MuteMode::All
    );
    assert_eq!(
        room_mute_mode(&pool, "user-1", r).await.unwrap(),
        MuteMode::ExceptMentions
    );
    let _ = notifications::room_mute_modes_for_user(&pool, "user-1")
        .await
        .unwrap();
}
