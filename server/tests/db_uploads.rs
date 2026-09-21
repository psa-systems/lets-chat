mod common;

async fn setup_chat_pool() -> sqlx::SqlitePool {
    common::chat_pool().await
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
    assert_eq!(a.duration_secs(), "1.500");
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

/// LC-925: `get_unfurl_image` reads cached bytes from the row instead of
/// re-fetching the remote origin on a second request. This exercises the
/// cache storage the route relies on: absent until `set_cached_image` is
/// called, then round-tripping the exact bytes and content type, and moving
/// to a fresh `image_fetched_at` on each `set_cached_image` call so the
/// route's TTL check has something to compare against.
#[tokio::test]
async fn link_preview_image_cache_round_trips() {
    let pool = setup_chat_pool().await;

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

    // No image cached yet: a fresh row has a URL but no bytes.
    let none = lets_chat::db::uploads::get_cached_image(&pool, "deadbeef")
        .await
        .unwrap();
    assert!(none.is_none());

    let bytes = vec![1u8, 2, 3, 4];
    lets_chat::db::uploads::set_cached_image(&pool, "deadbeef", "image/png", &bytes, i64::MAX)
        .await
        .unwrap();

    let cached = lets_chat::db::uploads::get_cached_image(&pool, "deadbeef")
        .await
        .unwrap()
        .expect("should exist after set_cached_image");
    assert_eq!(cached.content_type, "image/png");
    assert_eq!(cached.bytes, bytes);
    assert!(!cached.fetched_at.is_empty());

    // Re-fetching updates the bytes and content type in place.
    let bytes2 = vec![9u8, 9, 9];
    lets_chat::db::uploads::set_cached_image(&pool, "deadbeef", "image/webp", &bytes2, i64::MAX)
        .await
        .unwrap();
    let cached2 = lets_chat::db::uploads::get_cached_image(&pool, "deadbeef")
        .await
        .unwrap()
        .expect("should still exist after re-fetch");
    assert_eq!(cached2.content_type, "image/webp");
    assert_eq!(cached2.bytes, bytes2);
}

/// LC-985: the global quota refuses writes past the cap, and the sweep clears
/// bytes older than the TTL.
#[tokio::test]
async fn link_preview_image_cache_quota_and_expiry() {
    let pool = setup_chat_pool().await;
    for h in ["aa", "bb"] {
        lets_chat::db::uploads::upsert_link_preview(&pool, h, "https://e.com", None, None, None)
            .await
            .unwrap();
    }
    let db = lets_chat::db::uploads::set_cached_image;
    assert!(db(&pool, "aa", "image/png", &[0u8; 10], 15).await.unwrap());
    assert!(!db(&pool, "bb", "image/png", &[0u8; 10], 15).await.unwrap());
    assert!(lets_chat::db::uploads::get_cached_image(&pool, "bb")
        .await
        .unwrap()
        .is_none());
    // Rewriting the same row does not count its own old bytes.
    assert!(db(&pool, "aa", "image/png", &[0u8; 15], 15).await.unwrap());

    assert_eq!(
        lets_chat::db::uploads::clear_expired_cached_images(&pool, 86400)
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE link_previews SET image_fetched_at = datetime('now', '-2 days')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        lets_chat::db::uploads::clear_expired_cached_images(&pool, 86400)
            .await
            .unwrap(),
        1
    );
    assert!(lets_chat::db::uploads::get_cached_image(&pool, "aa")
        .await
        .unwrap()
        .is_none());
}
