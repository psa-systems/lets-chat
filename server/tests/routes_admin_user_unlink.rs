#![cfg(feature = "standalone")]
//! Integration tests for the admin "Unlink SSO" action on the users
//! admin page. Covers: AdminUser gating, successful unlink removes
//! the row + future find_user_by_sso returns None, no-op when the
//! user has no linked identity.

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
        let p = std::env::temp_dir().join(format!("lets-chat-admin-unlink-{}", std::process::id()));
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

async fn make_app_with_role(role: &str) -> (Router, String, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let actor_id = db::auth::create_user(&auth, "admin", "hash").await.unwrap();
    db::auth::set_user_role(&auth, &actor_id, role)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&actor_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &actor_id).await.unwrap();

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
    (routes::build_router(state), session, auth)
}

fn post(uri: &str, sess: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn admin_unlink_removes_sso_identity_row() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, "https://idp/", "sub-1", Some("a@x"), false)
        .await
        .unwrap();
    assert_eq!(
        db::sso::find_user_by_sso(&auth, "https://idp/", "sub-1")
            .await
            .unwrap()
            .as_deref(),
        Some(alice.as_str())
    );

    let res = app
        .clone()
        .oneshot(post(&format!("/admin/users/{}/sso/unlink", alice), &sess))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Row gone.
    assert!(db::sso::find_user_by_sso(&auth, "https://idp/", "sub-1")
        .await
        .unwrap()
        .is_none());
    // Returned HTML is the user-row fragment.
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("alice"));
}

#[tokio::test]
async fn admin_unlink_when_user_has_no_link_is_noop_returns_200() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let res = app
        .clone()
        .oneshot(post(&format!("/admin/users/{}/sso/unlink", alice), &sess))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_unlink_rejected_for_non_admin() {
    let (app, sess, auth) = make_app_with_role("user").await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, "https://idp/", "sub-1", None, false)
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(post(&format!("/admin/users/{}/sso/unlink", alice), &sess))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Row untouched.
    assert!(db::sso::find_user_by_sso(&auth, "https://idp/", "sub-1")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn list_renders_unlink_button_for_linked_users() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, "https://idp/", "sub-1", Some("a@x"), false)
        .await
        .unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/admin/users")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains(&format!("/admin/users/{}/sso/unlink", alice)));
    assert!(body.contains("Unlink"));
}

#[tokio::test]
async fn list_renders_no_password_warning_for_sso_only_user() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    // Create SSO-only user via the auto-provision helper (sets password_hash = NULL).
    let uid = db::sso::create_user_from_sso(
        &auth,
        db::sso::CreateUserFromSso {
            issuer: "https://idp/",
            subject: "sso-sub",
            email: Some("sso@example.com"),
            preferred_username: Some("ssouser"),
            display_name: Some("SSO User"),
        },
    )
    .await
    .unwrap();
    let _ = uid;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/admin/users")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(
        body.contains("No password set"),
        "expected warning text for SSO-only user, got: {}",
        &body[..body.len().min(2000)]
    );
}
