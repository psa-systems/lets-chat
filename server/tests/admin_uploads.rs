#![cfg(feature = "standalone")]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-admin-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        std::fs::create_dir_all(p.join("uploads")).unwrap();
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
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
            include_str!("../migrations/auth/0019_api_tokens.sql"),
            include_str!("../migrations/auth/0020_bots.sql"),
            include_str!("../migrations/auth/0021_user_dnd.sql"),
            include_str!("../migrations/auth/0022_mobile_push.sql"),
            include_str!("../migrations/auth/0023_notify_email_activity.sql"),
            include_str!("../migrations/auth/0024_user_locale.sql"),
            include_str!("../migrations/auth/0025_user_theme.sql"),
            include_str!("../migrations/auth/0026_user_density.sql"),
        ],
        "chat" => vec![
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
            include_str!("../migrations/chat/0057_enclave_msg_rate_limit.sql"),
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_anti_spam.sql"),
            include_str!("../migrations/settings/0005_imap_inbox_config.sql"),
            include_str!("../migrations/settings/0006_drop_smtp_settings.sql"),
            include_str!("../migrations/settings/0007_imap_dead_letter_folder.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn make_app(username: &str, role: &str) -> (Router, String, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, username, "hash")
        .await
        .unwrap();
    db::auth::set_user_role(&auth, &user_id, role)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    (routes::build_router(state), session, chat)
}

async fn post_status(app: Router, sess: Option<&str>, uri: &str) -> StatusCode {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    app.oneshot(req).await.unwrap().status()
}

fn write_png(path: &std::path::Path, size: u32) {
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(size, size, image::Rgba([10, 200, 30, 255]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, size, size, image::ExtendedColorType::Rgba8)
        .unwrap();
    std::fs::write(path, &buf).unwrap();
}

#[tokio::test]
async fn purge_orphans_removes_stale_row() {
    let (app, sess, chat) = make_app("admin-purge", "admin").await;
    let id = db::uploads::insert_upload(
        &chat,
        "user-x",
        "f.png",
        "image/png",
        10,
        "admin-purge-test.png",
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE file_uploads SET created_at = datetime('now', '-25 hours') WHERE id = ?")
        .bind(id)
        .execute(&chat)
        .await
        .unwrap();

    let status = post_status(app, Some(&sess), "/admin/uploads/purge-orphans").await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "expected redirect on success"
    );
    assert!(
        db::uploads::get_upload(&chat, id).await.unwrap().is_none(),
        "stale orphan should be gone after purge",
    );
}

#[tokio::test]
async fn regenerate_thumbnails_creates_missing_preview() {
    let (app, sess, chat) = make_app("admin-regen", "admin").await;
    let storage = "admin-regen-test.png";
    let original_path = db::uploads_dir().join(storage);
    write_png(&original_path, 200);
    let preview_name = lets_chat::uploads::preview_storage_name(storage);
    let preview_path = db::uploads_dir().join(&preview_name);
    let _ = std::fs::remove_file(&preview_path);
    assert!(
        !preview_path.exists(),
        "preconditions: preview must be absent"
    );

    db::uploads::insert_upload(&chat, "user-x", "f.png", "image/png", 1234, storage, None)
        .await
        .unwrap();

    let status = post_status(app, Some(&sess), "/admin/uploads/regenerate-thumbnails").await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "expected redirect on success"
    );
    assert!(
        preview_path.exists(),
        "preview should have been generated at {}",
        preview_path.display(),
    );
}

#[tokio::test]
async fn admin_uploads_anonymous_redirects_to_login() {
    let (app, _sess, _chat) = make_app("admin-anon", "admin").await;
    // Drop the session: hit the endpoint without a cookie. AdminUser
    // returns a 303 to /login when there is no User in extensions.
    let status = post_status(app.clone(), None, "/admin/uploads/purge-orphans").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let status = post_status(app, None, "/admin/uploads/regenerate-thumbnails").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn admin_uploads_non_admin_rejected_with_403() {
    let (app, sess, _chat) = make_app("regular-user", "user").await;
    let status = post_status(app.clone(), Some(&sess), "/admin/uploads/purge-orphans").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let status = post_status(app, Some(&sess), "/admin/uploads/regenerate-thumbnails").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
