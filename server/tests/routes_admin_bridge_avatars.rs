//! LC-207: admin bridge-avatar cache diagnostic page.
//!
//! The `/admin/bridges/avatars` surface is standalone-only (the whole
//! `routes::admin` module is). File-scope feature gate per internal/CLAUDE.md
//! Phase-24 category 3 so saas mode does not compile / run these.
#![cfg(feature = "standalone")]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::bridge_avatar_proxies as p;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [207u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("lc-207-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        db::set_data_dir(dir.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
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
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
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
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        admin_session,
        chat: chat_for_test,
    }
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Seed a `failed` row with a specific foreign_url + reason.
async fn seed_failed(chat: &SqlitePool, hash: &str, url: &str, reason: &str) {
    p::upsert_pending(chat, hash, url).await.unwrap();
    p::mark_failed(chat, hash, reason).await.unwrap();
}

#[tokio::test]
async fn empty_cache_renders_page_with_empty_state() {
    let t = app().await;
    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges/avatars").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Bridge avatar cache"));
    assert!(body.contains("No failed bridge-avatar fetches."));
    // Oldest stat over an empty cache renders the "-" placeholder, never a
    // Rust Option discriminant.
    assert!(
        !body.contains("None"),
        "empty cache must not leak a None discriminant"
    );
}

#[tokio::test]
async fn failed_row_shows_host_in_cell_and_full_url_in_tooltip() {
    let t = app().await;
    // A realistic long Matrix media URL: host is short, path is sha-like.
    let url = "https://matrix.org/_matrix/media/v3/download/matrix.org/AbCdEf0123456789xyz";
    seed_failed(&t.chat, "h1", url, "http 404").await;

    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges/avatars").await;
    assert_eq!(status, StatusCode::OK);
    // Host-only in the scannable cell.
    assert!(
        body.contains(">matrix.org</span>"),
        "host should render as the cell text"
    );
    // Full URL available in the title= tooltip (host-only is a summary
    // affordance, not a secret - the full URL is one hover away).
    assert!(
        body.contains(&format!("title=\"{url}\"")),
        "full URL should ride in the title tooltip"
    );
    assert!(body.contains("http 404"), "failure reason shown");
}

#[tokio::test]
async fn unparseable_url_shows_sentinel_in_cell_raw_in_tooltip() {
    // Pins the <unparseable> sentinel choice specifically: a truncated raw
    // string would read like a host and hide the anomaly. A future refactor
    // back to truncate-fallback must fail this test.
    let t = app().await;
    let raw = "not a url at all";
    seed_failed(&t.chat, "h2", raw, "magic-byte sniff returned no type").await;

    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges/avatars").await;
    assert_eq!(status, StatusCode::OK);
    // Askama HTML-escapes the angle brackets.
    assert!(
        body.contains("&lt;unparseable&gt;"),
        "an unparseable foreign_url must render the sentinel, not a truncated host-lookalike"
    );
    // The raw value still rides in the tooltip for diagnosis.
    assert!(
        body.contains(&format!("title=\"{raw}\"")),
        "raw unparseable URL preserved in the title tooltip"
    );
}

#[tokio::test]
async fn stale_pending_anomaly_banner_appears() {
    let t = app().await;
    // A pending row back-dated past the 4200s (70-min) display threshold.
    p::upsert_pending(&t.chat, "p1", "https://x.test/p1")
        .await
        .unwrap();
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-90 minutes') WHERE hash = 'p1'")
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges/avatars").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Anomaly:"),
        "a stale-pending row should surface the anomaly banner"
    );
}
