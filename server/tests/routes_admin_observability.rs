#![cfg(feature = "standalone")]
//! LC-207-OBSERVABILITY (#278): the admin settings page surfaces email-ingress
//! poll health, recent drops, and retention-sweep status. Standalone-only
//! because `routes::admin` is `#[cfg(feature = "standalone")]`.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [29u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-obs-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    app: Router,
    admin_session: String,
    chat: SqlitePool,
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

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
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
        chat: chat_for_test,
        settings: settings_for_test,
    }
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
async fn settings_page_renders_seeded_ingress_health_and_drops() {
    let f = setup().await;
    db::imap_poll_status::record_success(&f.settings, 7, 5, 1)
        .await
        .unwrap();
    db::email_ingress_drops::record(&f.chat, "rate_limited", Some(42), "inbox 4, retry 30s")
        .await
        .unwrap();

    let html = get_settings_html(&f.app, &f.admin_session).await;
    assert!(
        html.contains("Email ingress health"),
        "ingress section heading"
    );
    // Last-tick counts render as "fetched / posted / dropped".
    assert!(
        html.contains("7 / 5 / 1"),
        "last-tick counts; html had:\n{html}"
    );
    // The drop shows up in the by-reason summary and the recent-drops table.
    assert!(html.contains("rate_limited"), "drop reason");
    assert!(html.contains("inbox 4, retry 30s"), "drop detail");
    // A seeded status means the not-run placeholder must NOT appear.
    assert!(
        !html.contains("has not run yet"),
        "not-run placeholder must be hidden once a poll status exists",
    );
}

#[tokio::test]
async fn settings_page_shows_placeholders_when_no_status() {
    let f = setup().await;
    let html = get_settings_html(&f.app, &f.admin_session).await;
    // No poll status yet -> the ingress not-run placeholder shows.
    assert!(
        html.contains("has not run yet"),
        "ingress not-run placeholder",
    );
    // Retention sweep section renders; the flag is unset in tests, so the
    // disabled affordance (with the env-var hint) shows.
    assert!(
        html.contains("Message retention sweep"),
        "retention heading"
    );
    assert!(
        html.contains("LETS_CHAT_RETENTION_SWEEP_ENABLED"),
        "retention disabled hint names the env var",
    );
}
