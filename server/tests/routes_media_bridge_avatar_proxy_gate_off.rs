//! LC-78-AVATAR-PROXY: gate-off behavior. Setting
//! `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` makes the proxy GET
//! endpoint 404 every hash (no fingerprinting between "feature off" and
//! "hash unknown"). Lives in its own binary so the process-global env var
//! cannot race other tests in cargo's parallel runner.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [29u8; 32];
const HASH64: &str = "0011223344556677889900112233445566778899001122334455667788990011";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-bavoff-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

#[tokio::test]
async fn gate_off_returns_404_for_every_hash() {
    ensure_tempdir();
    std::env::set_var("LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED", "false");
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
    // Stage an OK row that WOULD normally serve, so the 404 we get back
    // proves the gate is what's blocking (not a missing row / file).
    let path = db::bridge_avatars_dir().join(HASH64);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"PNGBYTES").unwrap();
    db::bridge_avatar_proxies::upsert_pending(&chat, HASH64, "https://x.test/g")
        .await
        .unwrap();
    db::bridge_avatar_proxies::mark_ok(&chat, HASH64, "image/png", 8)
        .await
        .unwrap();
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
    let app = routes::build_router(state);
    let uri = format!("/media/bridge-avatar-proxy/{HASH64}");
    let req = Request::builder()
        .method(Method::GET)
        .uri(&uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    std::env::remove_var("LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED");
    assert_eq!(status, StatusCode::NOT_FOUND, "gate-off must 404 even on a row that would normally serve");
}
