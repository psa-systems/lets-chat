//! Phase 18: connection-lost UI smoke tests.
//!
//! Most of phase 18 is client-side JS lifecycle that requires a browser
//! harness this repo does not have. The two things we can verify
//! server-side without one are: (1) the banner partial is reachable in
//! its baseline `data-state="hidden"` shape via any layout-rendering
//! route, so a future template rename does not silently delete it; and
//! (2) the `<main id="main">` wrapper that the client-side soft-refresh
//! extracts via `select: '#main'` keeps existing.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-reconnect-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
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
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn app_with_logged_in_user() -> (Router, String) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let user_id = db::auth::create_user(&auth, "viewer", "hash")
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        // 2FA disabled keeps GET / off the TOTP-setup redirect path so we
        // can render the layout straight up. This test is about the
        // banner partial rendering, not auth flow.
        secret_key: None,
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
    };
    (routes::build_router(state), session)
}

async fn body_text(app: Router, session: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn home_includes_connection_status_banner_in_hidden_state() {
    let (app, session) = app_with_logged_in_user().await;
    let body = body_text(app, &session).await;
    assert!(
        body.contains(r#"id="lc-conn-status""#),
        "missing banner element id"
    );
    assert!(
        body.contains(r#"data-state="hidden""#),
        "banner did not render in baseline hidden state"
    );
}

#[tokio::test]
async fn home_keeps_main_wrapper_for_soft_refresh_select_target() {
    let (app, session) = app_with_logged_in_user().await;
    let body = body_text(app, &session).await;
    assert!(
        body.contains(r#"<main id="main""#),
        "<main id=\"main\"> wrapper missing - client soft-refresh uses select:'#main' to extract it on reconnect; renaming or removing it silently breaks reconnect recovery",
    );
}
