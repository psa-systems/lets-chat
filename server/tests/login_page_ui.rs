//! Integration tests for the /login page rendering: SSO buttons appear
//! when providers are enabled, password form is removed when
//! `LETS_CHAT_LOCAL_LOGIN_DISABLED` is on, and the OR divider only
//! shows when both forms are present. Per doc 10 section 6.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::sso_providers::{self, InsertProvider};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-login-ui-{}", std::process::id()));
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

async fn make_app(providers: &[(&str, &str)], local_login_disabled: bool) -> (Router, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    for (id, display_name) in providers {
        sso_providers::insert_provider(
            &auth,
            InsertProvider {
                id,
                kind: "oidc",
                display_name,
                issuer_url: &format!("https://idp-{id}/"),
                client_id: "c",
                client_secret_encrypted: b"s",
                scopes: "openid",
                attribute_map_json: "{}",
                allow_signup: false,
                auto_link_verified_email: true,
            },
        )
        .await
        .unwrap();
        sso_providers::set_provider_enabled(&auth, id, true)
            .await
            .unwrap();
    }
    let sso = lets_chat::sso::SsoProviders::load_enabled(&auth)
        .await
        .unwrap();

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
        sso,
        local_login_disabled,
    };
    (routes::build_router(state), auth)
}

async fn fetch_login(app: &Router) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn no_providers_renders_password_form_without_sso() {
    let (app, _) = make_app(&[], false).await;
    let body = fetch_login(&app).await;
    assert!(body.contains("name=\"username\""));
    assert!(body.contains("name=\"password\""));
    assert!(!body.contains("/auth/sso/"));
}

#[tokio::test]
async fn one_provider_renders_button_with_or_divider() {
    let (app, _) = make_app(&[("acme", "Acme SSO")], false).await;
    let body = fetch_login(&app).await;
    assert!(
        body.contains("name=\"username\""),
        "password form still shown"
    );
    assert!(body.contains("/auth/sso/acme/start"));
    assert!(body.contains("Sign in with Acme SSO"));
    // OR divider visible when both forms render.
    assert!(body.contains(">or<"));
}

#[tokio::test]
async fn multiple_providers_render_one_button_each() {
    let (app, _) = make_app(&[("acme", "Acme"), ("google", "Google Workspace")], false).await;
    let body = fetch_login(&app).await;
    assert!(body.contains("/auth/sso/acme/start"));
    assert!(body.contains("/auth/sso/google/start"));
    assert!(body.contains("Sign in with Acme"));
    assert!(body.contains("Sign in with Google Workspace"));
}

#[tokio::test]
async fn local_login_disabled_removes_password_form_but_keeps_sso() {
    let (app, _) = make_app(&[("acme", "Acme SSO")], true).await;
    let body = fetch_login(&app).await;
    assert!(!body.contains("name=\"username\""), "password form removed");
    assert!(!body.contains("name=\"password\""));
    assert!(body.contains("/auth/sso/acme/start"));
    // No OR divider when there's only the SSO column.
    assert!(!body.contains(">or<"));
}

#[tokio::test]
async fn local_login_disabled_with_no_providers_shows_neither() {
    // Operator misconfig - flag on but no providers configured. The
    // page still renders without crashing; the user sees only the
    // version footer and no sign-in path. Documented as a known
    // foot-gun in doc 10 section 8.
    let (app, _) = make_app(&[], true).await;
    let body = fetch_login(&app).await;
    assert!(!body.contains("name=\"username\""));
    assert!(!body.contains("/auth/sso/"));
}
