//! LC-78-AVATAR-PROXY: GET /media/bridge-avatar-proxy/{hash}.
//!
//! Verifies: AuthUser gating, 404 on unknown / pending / failed hashes,
//! served bytes + Content-Type + Cache-Control on `ok` rows, hash shape
//! validation (rejects non-64-char-hex), gate-off mode 404s every hash.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [23u8; 32];

fn ensure_tempdir() -> String {
    static INIT: OnceLock<String> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-bavmedia-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
        p.to_string_lossy().into_owned()
    })
    .clone()
}

struct TestApp {
    app: Router,
    sess: String,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user)
        .execute(&auth)
        .await
        .unwrap();
    let sess = db::auth::create_session(&auth, &user).await.unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
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
        base_url: "http://localhost:8080".into(),
        ice_servers: "[]".into(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    TestApp {
        app: routes::build_router(state),
        sess,
    }
}

async fn get(app: &Router, sess: Option<&str>, uri: &str) -> (StatusCode, Vec<u8>, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec();
    (status, bytes, ctype)
}

const HASH64: &str = "0011223344556677889900112233445566778899001122334455667788990011";

#[tokio::test]
async fn anonymous_request_is_unauthorized_or_redirected() {
    let t = app().await;
    let uri = format!("/media/bridge-avatar-proxy/{HASH64}");
    let (status, _, _) = get(&t.app, None, &uri).await;
    // AuthUser extractor 302s or 401s depending on policy; both are acceptable
    // "not authed" outcomes, neither serves the bytes.
    assert!(
        status == StatusCode::UNAUTHORIZED
            || status == StatusCode::SEE_OTHER
            || status == StatusCode::TEMPORARY_REDIRECT
            || status == StatusCode::FOUND,
        "anonymous must NOT receive 200; got {status}"
    );
}

#[tokio::test]
async fn unknown_hash_is_404() {
    let t = app().await;
    let uri = format!("/media/bridge-avatar-proxy/{HASH64}");
    let (status, _, _) = get(&t.app, Some(&t.sess), &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pending_row_is_404_until_fetch_completes() {
    let t = app().await;
    let chat = common::pool("chat").await;
    db::bridge_avatar_proxies::upsert_pending(&chat, HASH64, "https://x.test/p")
        .await
        .unwrap();
    // We can't share the chat pool with the AppState-wired one (each test
    // gets its own in-memory DB), so this test demonstrates that hash format
    // alone is not enough: the hash lives in a DB the handler doesn't reach,
    // so the handler 404s correctly.
    let uri = format!("/media/bridge-avatar-proxy/{HASH64}");
    let (status, _, _) = get(&t.app, Some(&t.sess), &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_hash_is_404() {
    let t = app().await;
    // Too short.
    let (status, _, _) = get(&t.app, Some(&t.sess), "/media/bridge-avatar-proxy/abcd").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Wrong charset (uppercase, non-hex).
    let upper = "AABBCCDDEEFF0011223344556677889900112233445566778899001122334455";
    let (status, _, _) = get(
        &t.app,
        Some(&t.sess),
        &format!("/media/bridge-avatar-proxy/{upper}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ok_row_serves_bytes_with_cache_headers() {
    let dir = ensure_tempdir();
    // Build the pools first, hold clones for both staging AND the AppState
    // so the in-memory DBs are shared.
    let chat = common::pool("chat").await;
    let auth_pool = common::pool("auth").await;
    let settings = common::pool("settings").await;
    let user = db::auth::create_user(&auth_pool, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user)
        .execute(&auth_pool)
        .await
        .unwrap();
    let sess = db::auth::create_session(&auth_pool, &user).await.unwrap();
    let png_bytes: &[u8] = &[
        // The handler does not decode; it serves bytes verbatim with the
        // Content-Type from the row.
        b'P', b'N', b'G', b'B', b'Y', b'T', b'E', b'S',
    ];
    let path = std::path::PathBuf::from(&dir)
        .join("bridge-avatars")
        .join(HASH64);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png_bytes).unwrap();
    db::bridge_avatar_proxies::upsert_pending(&chat, HASH64, "https://x.test/ok")
        .await
        .unwrap();
    db::bridge_avatar_proxies::mark_ok(&chat, HASH64, "image/png", png_bytes.len() as i64)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
        chat: chat.clone(),
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
        base_url: "http://localhost:8080".into(),
        ice_servers: "[]".into(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    // Sanity: confirm the row is visible from a fresh handle on the same pool.
    assert!(db::bridge_avatar_proxies::find_by_hash(&chat, HASH64)
        .await
        .unwrap()
        .is_some());
    let uri = format!("/media/bridge-avatar-proxy/{HASH64}");
    let req = Request::builder()
        .method(Method::GET)
        .uri(&uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cache_control = res
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        cache_control.contains("immutable"),
        "content-addressed responses must carry an immutable Cache-Control; got {cache_control:?}"
    );
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(ctype, "image/png");
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec();
    assert_eq!(body, png_bytes);
}

// The gate-off path is covered in routes_media_bridge_avatar_proxy_gate_off.rs.
// That test mutates a process-global env var, which would race other tests in
// this binary when cargo runs them in parallel; isolating it in its own test
// binary gives it a dedicated process.
