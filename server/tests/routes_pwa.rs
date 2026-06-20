//! LC-98: PWA install + service-worker plumbing.
//!
//! Covers the dynamically-served service worker (asset-version cache-name
//! substitution, JS content type, no-cache header) and the PWA <head>
//! wiring on an unauthenticated page (manifest link, theme-color, the
//! outbox client script). The offline-outbox queue logic itself lives in
//! the service worker / IndexedDB and is exercised in the browser, not
//! here.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-pwa-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

mod common;

async fn app() -> Router {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "testver".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
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
    };
    routes::build_router(state)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, header::HeaderMap, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn service_worker_served_with_version_substituted() {
    let app = app().await;
    let (status, headers, body) = get(&app, "/sw.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/javascript"
    );
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
    // The build's asset version is baked into the cache name so a new
    // deploy gets a fresh cache that activate() can evict.
    // The cache name is `'lc-shell-' + ASSET_VERSION`; the version is the
    // substituted constant, so assert the substituted value is present.
    assert!(
        body.contains("ASSET_VERSION = 'testver'"),
        "asset version should be substituted into the worker",
    );
    assert!(
        body.contains("lc-shell-"),
        "versioned cache name prefix present"
    );
    assert!(
        !body.contains("__ASSET_VERSION__"),
        "the version placeholder must be substituted",
    );
}

#[tokio::test]
async fn service_worker_keeps_push_and_outbox_handlers() {
    let app = app().await;
    let (_, _, body) = get(&app, "/sw.js").await;
    assert!(
        body.contains("addEventListener('push'"),
        "push handler kept"
    );
    assert!(
        body.contains("addEventListener('fetch'"),
        "fetch handler for offline + outbox present",
    );
    assert!(body.contains("lc-outbox"), "outbox IndexedDB queue present");
}

#[tokio::test]
async fn login_page_has_pwa_head_wiring() {
    let app = app().await;
    let (status, _, body) = get(&app, "/login").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("rel=\"manifest\""),
        "manifest link present in <head>",
    );
    assert!(
        body.contains("name=\"theme-color\""),
        "theme-color meta present",
    );
    assert!(
        body.contains("/assets/outbox.js"),
        "outbox client script loaded",
    );
}
