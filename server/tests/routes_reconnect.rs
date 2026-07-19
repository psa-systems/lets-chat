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

mod common;

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
    common::pool(name).await
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
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        // 2FA disabled keeps GET / off the TOTP-setup redirect path so we
        // can render the layout straight up. This test is about the
        // banner partial rendering, not auth flow.
        secret_key: None,
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
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
