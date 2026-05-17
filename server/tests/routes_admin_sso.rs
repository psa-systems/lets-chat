#![cfg(feature = "standalone")]
//! Integration tests for `/admin/sso/*` CRUD routes.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::sso_providers;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-admin-sso-{}", std::process::id()));
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

async fn make_app_with_role(role: &str) -> (Router, String, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, "admin", "hash").await.unwrap();
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
        auth: auth.clone(),
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([7u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        sso: lets_chat::sso::SsoProviders::default(),
    };
    (routes::build_router(state), session, auth)
}

fn req_get(uri: &str, sess: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    b.body(Body::empty()).unwrap()
}

fn req_post_form(uri: &str, sess: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn list_requires_admin() {
    let (app, sess, _) = make_app_with_role("user").await;
    let res = app
        .clone()
        .oneshot(req_get("/admin/sso", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_empty_renders_for_admin() {
    let (app, sess, _) = make_app_with_role("admin").await;
    let res = app
        .clone()
        .oneshot(req_get("/admin/sso", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("No SSO providers configured"));
}

#[tokio::test]
async fn create_inserts_encrypted_row_and_redirects() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let form = "id=acme&display_name=Acme&issuer_url=https%3A%2F%2Fidp.example%2F\
                &client_id=cli&client_secret=topsecret&scopes=openid+email\
                &allow_signup=1&auto_link_verified_email=1";
    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(loc, "/admin/sso/acme?flash=created");

    let row = sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.display_name, "Acme");
    assert!(row.allow_signup);
    assert!(!row.client_secret_encrypted.is_empty(), "secret encrypted");
    // Plaintext is not present in the encrypted blob.
    assert!(!row
        .client_secret_encrypted
        .windows("topsecret".len())
        .any(|w| w == b"topsecret"));
}

#[tokio::test]
async fn create_rejects_bad_slug() {
    let (app, sess, _) = make_app_with_role("admin").await;
    let form = "id=Acme!&display_name=x&issuer_url=https%3A%2F%2Fa%2F\
                &client_id=c&client_secret=s";
    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_rejects_missing_secret() {
    let (app, sess, _) = make_app_with_role("admin").await;
    let form = "id=acme&display_name=x&issuer_url=https%3A%2F%2Fa%2F\
                &client_id=c&client_secret=";
    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn edit_preserves_secret_when_field_blank() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    // Create first.
    let form = "id=acme&display_name=Acme&issuer_url=https%3A%2F%2Fidp%2F\
                &client_id=cli&client_secret=original";
    app.clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    let before = sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .unwrap()
        .client_secret_encrypted;

    // Edit with empty client_secret - must NOT rotate the stored value.
    let edit = "action=save&display_name=Acme+v2&issuer_url=https%3A%2F%2Fidp%2F\
                &client_id=cli&client_secret=&scopes=openid";
    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso/acme", &sess, edit))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let after = sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.display_name, "Acme v2");
    assert_eq!(after.client_secret_encrypted, before);
}

#[tokio::test]
async fn enable_then_disable_toggles_flags() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let form = "id=acme&display_name=Acme&issuer_url=https%3A%2F%2Fidp%2F\
                &client_id=cli&client_secret=s";
    app.clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    app.clone()
        .oneshot(req_post_form("/admin/sso/acme/enable", &sess, ""))
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .unwrap();
    assert!(row.is_enabled());
    app.clone()
        .oneshot(req_post_form("/admin/sso/acme/disable", &sess, ""))
        .await
        .unwrap();
    // Force disabled_at strictly above enabled_at to dodge same-second
    // SQL precision; the route handler uses strftime which can collide.
    sqlx::query("UPDATE sso_providers SET disabled_at = enabled_at + 1 WHERE id = ?")
        .bind("acme")
        .execute(&auth)
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .unwrap();
    assert!(!row.is_enabled());
}

#[tokio::test]
async fn delete_refuses_when_identities_reference_issuer() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let form = "id=acme&display_name=Acme&issuer_url=https%3A%2F%2Fidp%2F\
                &client_id=cli&client_secret=s";
    app.clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    // Insert a linked identity for the provider's issuer.
    let uid = db::auth::create_user(&auth, "linked", "h").await.unwrap();
    db::sso::link_sso_identity(&auth, &uid, "https://idp/", "sub-1", None, false)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso/acme/delete", &sess, ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    // Row still exists.
    assert!(sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn delete_succeeds_when_no_identities() {
    let (app, sess, auth) = make_app_with_role("admin").await;
    let form = "id=acme&display_name=Acme&issuer_url=https%3A%2F%2Fidp%2F\
                &client_id=cli&client_secret=s";
    app.clone()
        .oneshot(req_post_form("/admin/sso", &sess, form))
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(req_post_form("/admin/sso/acme/delete", &sess, ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(sso_providers::get_provider_by_id(&auth, "acme")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn edit_returns_404_for_missing_provider() {
    let (app, sess, _) = make_app_with_role("admin").await;
    let res = app
        .clone()
        .oneshot(req_get("/admin/sso/ghost", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
