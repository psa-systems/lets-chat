use sqlx::SqlitePool;

async fn setup_chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: &[&str] = &[
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
        include_str!("../migrations/chat/0020_quote_reply.sql"),
        include_str!("../migrations/chat/0021_enclave_invitations_enclave_idx.sql"),
        include_str!("../migrations/chat/0022_voice_messages.sql"),
        include_str!("../migrations/chat/0023_system_messages.sql"),
        include_str!("../migrations/chat/0024_voice_channel_flag.sql"),
        include_str!("../migrations/chat/0025_message_edits.sql"),
        include_str!("../migrations/chat/0026_room_categories.sql"),
        include_str!("../migrations/chat/0027_user_groups.sql"),
        include_str!("../migrations/chat/0028_room_role_overrides.sql"),
        include_str!("../migrations/chat/0029_room_posting_policy.sql"),
        include_str!("../migrations/chat/0030_room_docs_wiki.sql"),
        include_str!("../migrations/chat/0031_storage_quotas.sql"),
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
    ];
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn insert_and_get_upload_round_trip() {
    let pool = setup_chat_pool().await;

    let id = lets_chat::db::uploads::insert_upload(
        &pool,
        "user-a",
        "hello.png",
        "image/png",
        1234,
        "abc123.png",
        None,
    )
    .await
    .unwrap();

    let (row, room_id) = lets_chat::db::uploads::get_upload(&pool, id)
        .await
        .unwrap()
        .expect("should exist");
    assert_eq!(row.id, id);
    assert_eq!(row.uploader_id, "user-a");
    assert_eq!(row.filename, "hello.png");
    assert_eq!(row.mime_type, "image/png");
    assert_eq!(row.size_bytes, 1234);
    assert_eq!(row.storage_path, "abc123.png");
    assert_eq!(row.message_id, None);
    assert_eq!(room_id, None);
}

#[tokio::test]
async fn link_upload_to_message_promotes_orphan() {
    let pool = setup_chat_pool().await;

    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    let mid = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "look at this")
        .await
        .unwrap();

    let upload_id = lets_chat::db::uploads::insert_upload(
        &pool,
        "user-a",
        "p.png",
        "image/png",
        9,
        "abc.png",
        None,
    )
    .await
    .unwrap();
    lets_chat::db::uploads::link_upload_to_message(&pool, upload_id, mid)
        .await
        .unwrap();

    let (row, room_id) = lets_chat::db::uploads::get_upload(&pool, upload_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.message_id, Some(mid));
    assert_eq!(room_id, Some(room.id));
}

#[tokio::test]
async fn attachments_for_messages_groups_by_message_id() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    let m1 = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "first")
        .await
        .unwrap();
    let m2 = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "second")
        .await
        .unwrap();
    let m3 = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "third")
        .await
        .unwrap();

    for (mid, name) in [(m1, "a.png"), (m1, "b.png"), (m2, "c.pdf")] {
        let id = lets_chat::db::uploads::insert_upload(
            &pool,
            "user-a",
            name,
            "image/png",
            1,
            &format!("{name}-storage"),
            None,
        )
        .await
        .unwrap();
        lets_chat::db::uploads::link_upload_to_message(&pool, id, mid)
            .await
            .unwrap();
    }

    let map = lets_chat::db::uploads::attachments_for_messages(&pool, &[m1, m2, m3])
        .await
        .unwrap();
    assert_eq!(map.get(&m1).map(|v| v.len()), Some(2));
    assert_eq!(map.get(&m2).map(|v| v.len()), Some(1));
    assert!(!map.contains_key(&m3));

    let by_mid = lets_chat::db::uploads::attachments_for_message(&pool, m2)
        .await
        .unwrap();
    assert_eq!(by_mid.len(), 1);
    assert_eq!(by_mid[0].filename, "c.pdf");
    assert_eq!(by_mid[0].url, format!("/api/files/{}", by_mid[0].id));
}

#[tokio::test]
async fn waveform_round_trips_through_attachment_load() {
    let pool = setup_chat_pool().await;
    let room = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();
    let mid = lets_chat::db::chat::insert_message(&pool, room.id, "user-a", "")
        .await
        .unwrap();

    // A voice upload stores its waveform JSON; a plain upload stores NULL.
    let voice_id = lets_chat::db::uploads::insert_upload(
        &pool,
        "user-a",
        "voice-message.ogg",
        "audio/ogg",
        2048,
        "deadbeef.ogg",
        Some(r#"{"d":1.5,"p":[0.0,0.5,1.0]}"#),
    )
    .await
    .unwrap();
    lets_chat::db::uploads::link_upload_to_message(&pool, voice_id, mid)
        .await
        .unwrap();

    let atts = lets_chat::db::uploads::attachments_for_message(&pool, mid)
        .await
        .unwrap();
    assert_eq!(atts.len(), 1);
    let a = &atts[0];
    assert!(a.is_audio());
    assert_eq!(a.waveform.as_deref(), Some(&[0.0, 0.5, 1.0][..]));
    assert_eq!(a.waveform_csv(), "0.000,0.500,1.000");
    assert_eq!(a.voice_duration, Some(1.5));
    assert_eq!(a.duration_attr(), "1.500");
}

#[tokio::test]
async fn non_dm_member_cannot_access_dm_room_via_room_predicate() {
    let pool = setup_chat_pool().await;
    let dm = lets_chat::db::chat::create_dm_room(&pool, "dm-a-b", "user-a", "user-b")
        .await
        .unwrap();

    // Random user-c is not a member of the DM room.
    let allowed = lets_chat::db::chat::is_room_accessible(&pool, dm.id, "user-c", false)
        .await
        .unwrap();
    assert!(!allowed);

    // Members pass.
    let allowed_a = lets_chat::db::chat::is_room_accessible(&pool, dm.id, "user-a", false)
        .await
        .unwrap();
    let allowed_b = lets_chat::db::chat::is_room_accessible(&pool, dm.id, "user-b", false)
        .await
        .unwrap();
    assert!(allowed_a);
    assert!(allowed_b);
}

#[tokio::test]
async fn link_preview_cache_round_trips() {
    let pool = setup_chat_pool().await;

    let none = lets_chat::db::uploads::get_link_preview(&pool, "deadbeef")
        .await
        .unwrap();
    assert!(none.is_none());

    lets_chat::db::uploads::upsert_link_preview(
        &pool,
        "deadbeef",
        "https://example.com",
        Some("Title"),
        Some("Desc"),
        Some("https://example.com/img.png"),
    )
    .await
    .unwrap();

    let row = lets_chat::db::uploads::get_link_preview(&pool, "deadbeef")
        .await
        .unwrap()
        .expect("should exist after upsert");
    assert_eq!(row.url, "https://example.com");
    assert_eq!(row.title.as_deref(), Some("Title"));
    assert_eq!(row.description.as_deref(), Some("Desc"));
    assert_eq!(
        row.image_url.as_deref(),
        Some("https://example.com/img.png")
    );

    // Upsert overwrite.
    lets_chat::db::uploads::upsert_link_preview(
        &pool,
        "deadbeef",
        "https://example.com",
        Some("Title 2"),
        None,
        None,
    )
    .await
    .unwrap();
    let row = lets_chat::db::uploads::get_link_preview(&pool, "deadbeef")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.title.as_deref(), Some("Title 2"));
    assert!(row.description.is_none());
    assert!(row.image_url.is_none());
}
