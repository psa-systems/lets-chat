use lets_chat::db;
use sqlx::SqlitePool;

async fn chat_pool() -> SqlitePool {
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
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn make_enclave(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO enclaves (name, created_by) VALUES (?, 'system')")
        .bind(name)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn insert_and_list_round_trip() {
    let chat = chat_pool().await;
    let eid = make_enclave(&chat, "test").await;

    let id = db::custom_emojis::insert(&chat, eid, "party", "abc.png", "image/png", 1234, "u-1")
        .await
        .unwrap();
    let row = db::custom_emojis::get(&chat, id).await.unwrap().unwrap();
    assert_eq!(row.shortcode, "party");
    assert_eq!(row.storage_path, "abc.png");
    assert_eq!(row.mime_type, "image/png");
    assert_eq!(row.size_bytes, 1234);
    assert_eq!(row.enclave_id, eid);

    let listed = db::custom_emojis::list_for_enclave(&chat, eid)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].shortcode, "party");
}

#[tokio::test]
async fn shortcode_unique_per_enclave() {
    let chat = chat_pool().await;
    let e1 = make_enclave(&chat, "one").await;
    let e2 = make_enclave(&chat, "two").await;

    db::custom_emojis::insert(&chat, e1, "duck", "a.png", "image/png", 1, "u-1")
        .await
        .unwrap();
    // Same shortcode in a different enclave succeeds.
    db::custom_emojis::insert(&chat, e2, "duck", "b.png", "image/png", 1, "u-1")
        .await
        .unwrap();
    // Same shortcode in the same enclave fails on the UNIQUE constraint.
    let err = db::custom_emojis::insert(&chat, e1, "duck", "c.png", "image/png", 1, "u-1")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, sqlx::Error::Database(d) if d.is_unique_violation()),
        "expected unique violation, got {err:?}"
    );
}

#[tokio::test]
async fn refs_for_room_returns_enclave_emojis() {
    let chat = chat_pool().await;
    let eid = make_enclave(&chat, "scoped").await;
    db::custom_emojis::insert(&chat, eid, "yay", "a.png", "image/png", 1, "u-1")
        .await
        .unwrap();

    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap();
    let refs = db::custom_emojis::refs_for_room(&chat, room_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].shortcode, "yay");
}

#[tokio::test]
async fn refs_for_room_returns_empty_for_dm() {
    let chat = chat_pool().await;
    let dm_id = db::chat::create_room(&chat, "dm-a-b", None, "dm", None, None)
        .await
        .unwrap();
    let refs = db::custom_emojis::refs_for_room(&chat, dm_id)
        .await
        .unwrap();
    assert!(refs.is_empty());
}

#[tokio::test]
async fn delete_removes_row_and_does_not_affect_other_enclaves() {
    let chat = chat_pool().await;
    let e1 = make_enclave(&chat, "one").await;
    let e2 = make_enclave(&chat, "two").await;
    let id1 = db::custom_emojis::insert(&chat, e1, "duck", "a.png", "image/png", 1, "u-1")
        .await
        .unwrap();
    let id2 = db::custom_emojis::insert(&chat, e2, "duck", "b.png", "image/png", 1, "u-1")
        .await
        .unwrap();

    db::custom_emojis::delete(&chat, id1).await.unwrap();
    assert!(db::custom_emojis::get(&chat, id1).await.unwrap().is_none());
    assert!(db::custom_emojis::get(&chat, id2).await.unwrap().is_some());
}

#[tokio::test]
async fn enclave_delete_cascades_to_emojis() {
    let chat = chat_pool().await;
    let eid = make_enclave(&chat, "doomed").await;
    let id = db::custom_emojis::insert(&chat, eid, "rip", "a.png", "image/png", 1, "u-1")
        .await
        .unwrap();
    sqlx::query("DELETE FROM enclaves WHERE id=?")
        .bind(eid)
        .execute(&chat)
        .await
        .unwrap();
    // The row must be gone via the ON DELETE CASCADE in 0017.
    assert!(db::custom_emojis::get(&chat, id).await.unwrap().is_none());
}

#[tokio::test]
async fn refs_for_room_layers_in_globally_shared() {
    let chat = chat_pool().await;
    let e_room = make_enclave(&chat, "home").await;
    let e_shared = make_enclave(&chat, "sharer").await;
    let e_private = make_enclave(&chat, "private").await;

    db::enclave::set_share_emojis_globally(&chat, e_shared, true)
        .await
        .unwrap();

    db::custom_emojis::insert(&chat, e_room, "own", "a.png", "image/png", 1, "u-1")
        .await
        .unwrap();
    db::custom_emojis::insert(
        &chat,
        e_shared,
        "fromshared",
        "b.png",
        "image/png",
        1,
        "u-1",
    )
    .await
    .unwrap();
    db::custom_emojis::insert(&chat, e_private, "hidden", "c.png", "image/png", 1, "u-1")
        .await
        .unwrap();

    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(e_room))
        .await
        .unwrap();
    let refs = db::custom_emojis::refs_for_room(&chat, room_id)
        .await
        .unwrap();
    let codes: Vec<&str> = refs.iter().map(|r| r.shortcode.as_str()).collect();
    assert!(codes.contains(&"own"), "own missing: {codes:?}");
    assert!(codes.contains(&"fromshared"), "shared missing: {codes:?}");
    assert!(!codes.contains(&"hidden"), "private leaked: {codes:?}");
}

#[tokio::test]
async fn refs_for_room_own_wins_on_collision() {
    let chat = chat_pool().await;
    let e_room = make_enclave(&chat, "home").await;
    let e_shared = make_enclave(&chat, "sharer").await;
    db::enclave::set_share_emojis_globally(&chat, e_shared, true)
        .await
        .unwrap();

    let id_shared = db::custom_emojis::insert(&chat, e_shared, "dup", "a.png", "image/png", 1, "u")
        .await
        .unwrap();
    let id_own = db::custom_emojis::insert(&chat, e_room, "dup", "b.png", "image/png", 1, "u")
        .await
        .unwrap();

    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(e_room))
        .await
        .unwrap();
    let refs = db::custom_emojis::refs_for_room(&chat, room_id)
        .await
        .unwrap();
    let dup_ids: Vec<i64> = refs
        .iter()
        .filter(|r| r.shortcode == "dup")
        .map(|r| r.id)
        .collect();
    assert_eq!(dup_ids, vec![id_own]);
    assert_ne!(id_own, id_shared);
}

#[tokio::test]
async fn refs_for_dm_returns_only_globally_shared() {
    let chat = chat_pool().await;
    let e_shared = make_enclave(&chat, "sharer").await;
    let e_private = make_enclave(&chat, "private").await;
    db::enclave::set_share_emojis_globally(&chat, e_shared, true)
        .await
        .unwrap();
    db::custom_emojis::insert(&chat, e_shared, "yes", "a.png", "image/png", 1, "u")
        .await
        .unwrap();
    db::custom_emojis::insert(&chat, e_private, "no", "b.png", "image/png", 1, "u")
        .await
        .unwrap();

    let dm_id = db::chat::create_room(&chat, "dm-a-b", None, "dm", None, None)
        .await
        .unwrap();
    let refs = db::custom_emojis::refs_for_room(&chat, dm_id)
        .await
        .unwrap();
    let codes: Vec<&str> = refs.iter().map(|r| r.shortcode.as_str()).collect();
    assert_eq!(codes, vec!["yes"]);
}

#[tokio::test]
async fn set_share_emojis_globally_round_trips() {
    let chat = chat_pool().await;
    let eid = make_enclave(&chat, "test").await;
    let enclave = db::enclave::get_enclave(&chat, eid).await.unwrap().unwrap();
    assert!(!enclave.share_emojis_globally);

    db::enclave::set_share_emojis_globally(&chat, eid, true)
        .await
        .unwrap();
    let enclave = db::enclave::get_enclave(&chat, eid).await.unwrap().unwrap();
    assert!(enclave.share_emojis_globally);

    db::enclave::set_share_emojis_globally(&chat, eid, false)
        .await
        .unwrap();
    let enclave = db::enclave::get_enclave(&chat, eid).await.unwrap().unwrap();
    assert!(!enclave.share_emojis_globally);
}
