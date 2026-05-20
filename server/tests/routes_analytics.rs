#![cfg(feature = "standalone")]

//! LC-97: admin analytics dashboard. Covers the pre-aggregation math
//! (recompute_day counts, soft-delete / system-message exclusion), the
//! dashboard route rendering, the recompute button, and the admin auth
//! gate.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::analytics;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-analytics-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
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
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_anti_spam.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

struct Harness {
    app: Router,
    session: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn make_app(username: &str, role: &str) -> Harness {
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
    // enforce_2fa middleware 303s users with totp_enabled = 0; flip it so
    // authed admin requests reach the handler.
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
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
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    Harness {
        app: routes::build_router(state),
        session,
        auth,
        chat,
    }
}

async fn today(chat: &SqlitePool) -> String {
    sqlx::query_scalar("SELECT date('now')")
        .fetch_one(chat)
        .await
        .unwrap()
}

async fn get_status_body(app: Router, sess: Option<&str>, uri: &str) -> (StatusCode, String) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    let req = builder.body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post_form_status(app: Router, sess: Option<&str>, uri: &str, body: &str) -> StatusCode {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn recompute_day_counts_messages_dau_and_rooms() {
    let h = make_app("admin-rec", "admin").await;
    let day = today(&h.chat).await;

    let room = db::chat::create_room(&h.chat, "general", None, "public", None, None)
        .await
        .unwrap();
    // Two distinct senders, three messages today.
    db::chat::insert_message(&h.chat, room, "u1", "hi")
        .await
        .unwrap();
    db::chat::insert_message(&h.chat, room, "u1", "again")
        .await
        .unwrap();
    db::chat::insert_message(&h.chat, room, "u2", "yo")
        .await
        .unwrap();

    analytics::recompute_day(&h.auth, &h.chat, &day)
        .await
        .unwrap();

    let messages = analytics::series(&h.chat, analytics::METRIC_MESSAGES, &day, &day)
        .await
        .unwrap();
    assert_eq!(messages.last().unwrap().value, 3, "three messages today");

    let dau = analytics::series(&h.chat, analytics::METRIC_DAU, &day, &day)
        .await
        .unwrap();
    assert_eq!(dau.last().unwrap().value, 2, "two distinct senders");

    let rooms = analytics::series(&h.chat, analytics::METRIC_ACTIVE_ROOMS, &day, &day)
        .await
        .unwrap();
    assert_eq!(rooms.last().unwrap().value, 1, "one active room");
}

#[tokio::test]
async fn recompute_excludes_deleted_and_system_messages() {
    let h = make_app("admin-excl", "admin").await;
    let day = today(&h.chat).await;
    let room = db::chat::create_room(&h.chat, "general", None, "public", None, None)
        .await
        .unwrap();

    let keep = db::chat::insert_message(&h.chat, room, "u1", "real")
        .await
        .unwrap();
    let gone = db::chat::insert_message(&h.chat, room, "u1", "oops")
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&h.chat)
        .await
        .unwrap();
    // System message must not inflate the count.
    db::chat::insert_system_message(&h.chat, room, "u1", "u1 started a call")
        .await
        .unwrap();
    let _ = keep;

    analytics::recompute_day(&h.auth, &h.chat, &day)
        .await
        .unwrap();
    let messages = analytics::series(&h.chat, analytics::METRIC_MESSAGES, &day, &day)
        .await
        .unwrap();
    assert_eq!(
        messages.last().unwrap().value,
        1,
        "only the one live human message counts",
    );
}

#[tokio::test]
async fn backfill_records_signup_metric() {
    let h = make_app("admin-signup", "admin").await;
    let day = today(&h.chat).await;
    // The admin user created in make_app registered today, so signups >= 1.
    analytics::backfill(&h.auth, &h.chat, &day).await.unwrap();
    let signups = analytics::series(&h.chat, analytics::METRIC_SIGNUPS, &day, &day)
        .await
        .unwrap();
    assert!(
        signups.last().map(|p| p.value).unwrap_or(0) >= 1,
        "today's signup count includes the admin",
    );
}

#[tokio::test]
async fn dashboard_renders_for_admin() {
    let h = make_app("admin-view", "admin").await;
    let (status, body) = get_status_body(h.app, Some(&h.session), "/admin/analytics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Daily active users"), "metric card present");
    assert!(
        body.contains("Retention by signup cohort"),
        "retention present"
    );
    assert!(body.contains("<svg"), "inline svg chart rendered");
}

#[tokio::test]
async fn recompute_button_redirects() {
    let h = make_app("admin-btn", "admin").await;
    let status = post_form_status(
        h.app,
        Some(&h.session),
        "/admin/analytics/recompute",
        "days=30",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn dashboard_rejects_non_admin_and_anonymous() {
    let h = make_app("plain-user", "user").await;
    let (status, _) = get_status_body(h.app.clone(), Some(&h.session), "/admin/analytics").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-admin blocked");

    let (status, _) = get_status_body(h.app, None, "/admin/analytics").await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "anonymous redirected to login"
    );
}
