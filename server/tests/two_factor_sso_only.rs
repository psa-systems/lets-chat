#![cfg(feature = "standalone")]
//! Integration tests for the SSO-only-user notice on /settings/2fa/setup.
//! Doc 10 section 7: users with NULL password_hash see an explanation
//! page; the enrollment GET and POST routes refuse the local TOTP
//! flow because the IdP is their authenticator.

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
        let p = std::env::temp_dir().join(format!("lets-chat-2fa-ssoonly-{}", std::process::id()));
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

async fn make_app() -> (Router, SqlitePool) {
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

        local_login_disabled: false,
    };
    (routes::build_router(state), auth)
}

async fn sign_in_as(auth: &SqlitePool, user_id: &str) -> String {
    db::auth::create_session(auth, user_id).await.unwrap()
}

#[tokio::test]
async fn sso_only_user_get_setup_renders_notice() {
    let (app, auth) = make_app().await;
    let uid = db::sso::create_user_from_sso(
        &auth,
        db::sso::CreateUserFromSso {
            issuer: "https://idp/",
            subject: "sub-1",
            email: Some("sso@example.com"),
            preferred_username: Some("ssouser"),
            display_name: Some("SSO User"),
        },
    )
    .await
    .unwrap();
    let sess = sign_in_as(&auth, &uid).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/settings/2fa/setup")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(
        body.contains("identity provider"),
        "expected SSO-only notice, got: {}",
        &body[..body.len().min(2000)]
    );
    // No QR enrollment form.
    assert!(!body.contains("qr_base64"));
}

#[tokio::test]
async fn sso_only_user_post_setup_returns_forbidden() {
    let (app, auth) = make_app().await;
    let uid = db::sso::create_user_from_sso(
        &auth,
        db::sso::CreateUserFromSso {
            issuer: "https://idp/",
            subject: "sub-2",
            email: None,
            preferred_username: Some("ssouser2"),
            display_name: None,
        },
    )
    .await
    .unwrap();
    let sess = sign_in_as(&auth, &uid).await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/2fa/setup")
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("code=123456"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // totp_enabled stays false.
    let record = db::auth::find_user_by_id(&auth, &uid)
        .await
        .unwrap()
        .unwrap();
    assert!(!record.totp_enabled);
}

#[tokio::test]
async fn local_user_get_setup_still_shows_qr() {
    let (app, auth) = make_app().await;
    let uid = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let sess = sign_in_as(&auth, &uid).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/settings/2fa/setup")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    // The local-password user gets the normal enrollment page, which
    // embeds a base64 QR image.
    assert!(
        body.contains("data:image"),
        "expected QR enrollment, got SSO-only notice"
    );
}
