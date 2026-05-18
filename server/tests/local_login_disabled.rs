#![cfg(feature = "standalone")]
//! Integration tests for the LETS_CHAT_LOCAL_LOGIN_DISABLED kill
//! switch. With the flag on, every password-related route returns
//! 404 regardless of credentials; with the flag off, the same routes
//! behave normally (sanity check / regression guard).

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
        let p = std::env::temp_dir().join(format!("lets-chat-killsw-{}", std::process::id()));
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
            include_str!("../migrations/auth/0016_sso_identities.sql"),
            include_str!("../migrations/auth/0017_sso_providers.sql"),
            include_str!("../migrations/auth/0018_sso_flows_provider.sql"),
            include_str!("../migrations/auth/0019_sso_group_mappings.sql"),
            include_str!("../migrations/auth/0020_session_tenant.sql"),
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

async fn make_app(local_login_disabled: bool) -> (Router, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([1u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        sso: lets_chat::sso::SsoProviders::default(),
        local_login_disabled,
    };
    (routes::build_router(state), auth)
}

fn post(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn post_login_404s_when_disabled() {
    let (app, auth) = make_app(true).await;
    db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let res = app
        .clone()
        .oneshot(post("/login", "username=alice&password=anything"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_register_404s_when_disabled() {
    let (app, _) = make_app(true).await;
    let res = app.clone().oneshot(get("/register")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_register_404s_when_disabled() {
    let (app, _) = make_app(true).await;
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            "username=newuser&password=hunter22&password_confirm=hunter22",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn forgot_and_reset_routes_404_when_disabled() {
    let (app, _) = make_app(true).await;
    for uri in ["/forgot"] {
        let res = app.clone().oneshot(get(uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    let res = app.clone().oneshot(get("/reset/some-token")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settings_password_404s_when_disabled() {
    let (app, auth) = make_app(true).await;
    let uid = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    // Force totp_enabled so the auth middleware doesn't redirect to
    // 2FA setup before the kill-switch gate runs.
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let sess = db::auth::create_session(&auth, &uid).await.unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/settings/password")
                .header(header::COOKIE, format!("session={sess}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "current_password=hash&new_password=newpassword12345&new_password_confirm=newpassword12345",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_login_still_renders_with_flag_on() {
    // The /login page stays mounted; L21 will hide the password form
    // conditionally. The kill switch only 404s the POST and the
    // password-recovery routes.
    let (app, _) = make_app(true).await;
    let res = app.clone().oneshot(get("/login")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn post_login_works_with_flag_off() {
    // Regression guard: flag-off must not change the existing behaviour.
    let (app, auth) = make_app(false).await;
    db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let res = app
        .clone()
        .oneshot(post("/login", "username=alice&password=wrong"))
        .await
        .unwrap();
    // The wrong-password path renders the login form again with an
    // inline error (422 unprocessable entity per the existing handler);
    // the assertion here is just "not a 404," i.e. the kill switch is
    // off and the route is reachable.
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
