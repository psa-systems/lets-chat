//! Integration test for the admin "Send test email" route added in
//! phase 22 task 2.
//!
//! The admin router is mounted only under the `standalone` feature; in
//! `saas` mode the parent SaaS app owns user/role management and the
//! admin routes do not exist. Gate the entire test binary so it compiles
//! to a no-op under `saas`, matching how the upstream admin code path
//! is gated in `routes/mod.rs`.
#![cfg(feature = "standalone")]

//!
//! The route handler reads `state.email_client` for the actual send;
//! production builds wire a `LettreEmailClient`, tests wire a
//! `MockEmailClient` here and assert on its recorded sends. The handler
//! also checks that the SMTP row in settings.db has a non-empty
//! `from_address`; we seed that via `db::smtp_settings::save` before
//! the POST so the path matches the documented operator workflow
//! (save config, restart, click test).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::smtp_settings::{SmtpConfigInput, TlsMode};
use lets_chat::email::{EmailClient, MockEmailClient};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-smtp-tests-{}", std::process::id()));
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
            include_str!("../migrations/auth/0010_digest_columns.sql"),
            include_str!("../migrations/auth/0011_user_email.sql"),
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
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_smtp_settings.sql"),
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
    mock: Arc<MockEmailClient>,
}

async fn build_harness(seed_smtp_from: Option<&str>) -> Harness {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let user_id = db::auth::create_user(&auth, "admin", "hash").await.unwrap();
    db::auth::set_user_role(&auth, &user_id, "admin")
        .await
        .unwrap();
    // The enforce_2fa_enrollment middleware redirects any user with
    // `totp_enabled = false` to /settings/2fa/setup whenever the
    // server has a secret key configured (which we need for SMTP
    // encryption in this test). Set the flag so admin routes are
    // reachable; we are not exercising the 2FA challenge flow here.
    sqlx::query("UPDATE users SET totp_enabled = 1 WHERE id = ?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let key = [42u8; 32];
    if let Some(from) = seed_smtp_from {
        db::smtp_settings::save(
            &settings,
            &key,
            &SmtpConfigInput {
                host: "smtp.example.com".into(),
                port: 587,
                username: None,
                password: Some("test-pass".into()),
                from_address: from.into(),
                tls_mode: TlsMode::StartTls,
            },
        )
        .await
        .unwrap();
    }

    let mock = Arc::new(MockEmailClient::default());
    let mock_as_client: Arc<dyn EmailClient> = mock.clone();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new(key)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        email_client: Some(mock_as_client),
    };
    Harness {
        app: routes::build_router(state),
        session,
        mock,
    }
}

async fn post_form(app: Router, session: &str, path: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::COOKIE, format!("session={session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn test_email_button_invokes_email_client_with_form_recipient() {
    let h = build_harness(Some("noreply@example.com")).await;
    let (status, html) = post_form(
        h.app,
        &h.session,
        "/admin/settings/smtp/test",
        "test_to=ops@example.com",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sent = h.mock.taken();
    assert_eq!(
        sent.len(),
        1,
        "expected exactly one send, got {}",
        sent.len()
    );
    let msg = &sent[0];
    assert_eq!(msg.to, "ops@example.com");
    assert_eq!(msg.from, "noreply@example.com");
    assert!(
        msg.subject.contains("SMTP test"),
        "subject should label this as a test, got {:?}",
        msg.subject
    );
    assert!(
        msg.text_body.contains("admin"),
        "text body should mention the triggering admin"
    );
    assert!(
        html.contains("Test email sent"),
        "page should render the success banner"
    );
}

#[tokio::test]
async fn test_email_button_rejects_blank_recipient() {
    let h = build_harness(Some("noreply@example.com")).await;
    let (_, html) = post_form(h.app, &h.session, "/admin/settings/smtp/test", "test_to=").await;
    assert_eq!(
        h.mock.taken().len(),
        0,
        "should not send on blank recipient"
    );
    assert!(
        html.contains("Recipient address is required"),
        "expected explicit error message in banner"
    );
}

#[tokio::test]
async fn test_email_button_surfaces_transport_failure() {
    let h = build_harness(Some("noreply@example.com")).await;
    *h.mock.fail_next.lock().unwrap() = true;
    let (_, html) = post_form(
        h.app,
        &h.session,
        "/admin/settings/smtp/test",
        "test_to=ops@example.com",
    )
    .await;
    assert!(
        html.contains("Send failed"),
        "expected failure banner verbatim"
    );
    assert!(
        html.contains("forced failure"),
        "expected underlying error text to surface"
    );
}

#[tokio::test]
async fn test_email_button_rejects_when_from_address_empty() {
    // SMTP row exists but from_address is blank: handler should refuse
    // rather than send a message with an empty From header.
    let h = build_harness(Some("")).await;
    let (_, html) = post_form(
        h.app,
        &h.session,
        "/admin/settings/smtp/test",
        "test_to=ops@example.com",
    )
    .await;
    assert_eq!(h.mock.taken().len(), 0);
    assert!(
        html.contains("From address is empty"),
        "expected the from-empty error banner"
    );
}
