#![cfg(feature = "standalone")]
//! LC-77 admin settings test for IMAP poll configuration.
//!
//! Verifies the `/admin/settings/imap` POST handler:
//! - Saves all fields, sealing the password via crypto::seal/open.
//! - Empty password input preserves the existing sealed value.
//! - Non-empty password input re-seals and overwrites.
//! - Plaintext password never appears in the GET /admin/settings render.
//! - Non-admin gets 4xx (cookie-auth gate).

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [13u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-imap-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    app: Router,
    admin_session: String,
    user_session: String,
    settings: SqlitePool,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();

    let regular = db::auth::create_user(&auth, "regular", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&regular)
        .execute(&auth)
        .await
        .unwrap();
    let user_session = db::auth::create_session(&auth, &regular).await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let settings_for_test = settings.clone();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    Fixture {
        app: routes::build_router(state),
        admin_session,
        user_session,
        settings: settings_for_test,
    }
}

async fn post_imap(app: &Router, session: &str, form_body: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/settings/imap")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(form_body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get_settings_html(app: &Router, session: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/admin/settings")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn first_save_seals_password_and_round_trips_via_read() {
    let f = setup().await;
    let status = post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=imap.example.com\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=mailer\
         &imap_password=hunter2-very-secret\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    assert!(status.is_redirection(), "got {status:?}");

    // db::imap_config::read round-trips the plaintext password we just
    // posted, proving the seal worked end-to-end.
    let cfg = db::imap_config::read(&f.settings, &SECRET)
        .await
        .unwrap()
        .expect("imap_inbox_config row");
    assert_eq!(cfg.host, "imap.example.com");
    assert_eq!(cfg.port, 993);
    assert!(cfg.tls);
    assert_eq!(cfg.username, "mailer");
    assert_eq!(cfg.password, "hunter2-very-secret");
    assert_eq!(cfg.folder, "INBOX");
    assert_eq!(cfg.ingress_domain.as_deref(), Some("mail.example.com"));
    assert!(cfg.enabled);

    // The on-disk row carries the SEALED bytes, not the plaintext.
    let raw_pw: Vec<u8> =
        sqlx::query_scalar("SELECT password_encrypted FROM imap_inbox_config WHERE id = 1")
            .fetch_one(&f.settings)
            .await
            .unwrap();
    assert!(!raw_pw.is_empty(), "ciphertext should be non-empty");
    let plaintext_bytes = b"hunter2-very-secret";
    assert!(
        raw_pw
            .windows(plaintext_bytes.len())
            .all(|w| w != plaintext_bytes),
        "plaintext password substring must not appear in the sealed BLOB",
    );

    // The GET /admin/settings render must never echo the plaintext password.
    let html = get_settings_html(&f.app, &f.admin_session).await;
    assert!(
        !html.contains("hunter2-very-secret"),
        "settings page must not render the plaintext password",
    );
    assert!(
        html.contains("leave blank to keep existing"),
        "password input must show the keep-existing placeholder when configured",
    );
}

#[tokio::test]
async fn empty_password_preserves_existing_sealed_value() {
    let f = setup().await;
    // First save: seed the row with a known password.
    post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=h\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=original-pw\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;

    // Second save: change the host but leave the password empty.
    let status = post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=updated-host\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    assert!(status.is_redirection(), "got {status:?}");

    let cfg = db::imap_config::read(&f.settings, &SECRET)
        .await
        .unwrap()
        .expect("imap_inbox_config row");
    assert_eq!(cfg.host, "updated-host", "host should have changed");
    assert_eq!(
        cfg.password, "original-pw",
        "empty password input must NOT clear the existing sealed value",
    );
}

#[tokio::test]
async fn non_empty_password_overwrites_existing_sealed_value() {
    let f = setup().await;
    post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=h\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=first-pw\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    let status = post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=h\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=second-pw\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    assert!(status.is_redirection());
    let cfg = db::imap_config::read(&f.settings, &SECRET)
        .await
        .unwrap()
        .expect("imap_inbox_config row");
    assert_eq!(cfg.password, "second-pw");
}

#[tokio::test]
async fn non_admin_forbidden_from_imap_settings_post() {
    let f = setup().await;
    let status = post_imap(
        &f.app,
        &f.user_session,
        "imap_host=h\
         &imap_port=993\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=pw\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    assert_ne!(status, StatusCode::SEE_OTHER);
    assert_ne!(status, StatusCode::FOUND);
    assert!(
        status == StatusCode::FORBIDDEN
            || status == StatusCode::UNAUTHORIZED
            || status.is_redirection()
                && status != StatusCode::SEE_OTHER
                && status != StatusCode::FOUND,
        "non-admin should not get the success redirect; got {status:?}",
    );

    // No row was written.
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM imap_inbox_config")
        .fetch_one(&f.settings)
        .await
        .unwrap();
    assert_eq!(row_count, 0);
}

#[tokio::test]
async fn invalid_port_returns_bad_request() {
    let f = setup().await;
    let status = post_imap(
        &f.app,
        &f.admin_session,
        "imap_host=h\
         &imap_port=not-a-number\
         &imap_tls=1\
         &imap_username=u\
         &imap_password=pw\
         &imap_folder=INBOX\
         &imap_ingress_domain=mail.example.com\
         &imap_enabled=1",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
