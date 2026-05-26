use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-inbox-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    session: String,
    user_id: String,
    peer_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    let peer_id = db::auth::create_user(&auth, "peer", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    let peer_session = db::auth::create_session(&auth, &peer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        session,
        user_id,
        peer_session,
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
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body =
        String::from_utf8(to_bytes(resp.into_body(), 10 << 20).await.unwrap().to_vec()).unwrap();
    (status, body)
}

#[tokio::test]
async fn empty_inbox_renders_all_caught_up() {
    let t = app().await;
    let (status, body) = get(&t.app, &t.session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("All caught up"),
        "expected empty-state copy, got: {body}"
    );
}

// LC-179: the inbox carries the hidden refresh bar the WS reveal targets, so a
// new unread surfaces a "refresh" affordance without clobbering infinite-scroll.
#[tokio::test]
async fn inbox_has_hidden_refresh_bar() {
    let t = app().await;
    let (status, body) = get(&t.app, &t.session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"id="lc-inbox-refresh""#),
        "inbox must carry the refresh-bar id the WS reveal targets",
    );
    // Page-rendered bar starts hidden (only the WS reveal un-hides it).
    assert!(
        body.contains("id=\"lc-inbox-refresh\" type=\"button\" hidden"),
        "the page-rendered refresh bar must start hidden",
    );
}

#[tokio::test]
async fn unread_message_appears_in_inbox() {
    let t = app().await;
    // Peer sends a message in room 1 (General).
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.peer_session))
        .body(Body::from("body=hello+from+peer&file_id="))
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = t.user_id;
    let _ = t.chat;

    let (status, body) = get(&t.app, &t.session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("hello from peer"),
        "expected peer message in inbox, got: {body}"
    );
    assert!(
        body.contains("/room/1#msg-"),
        "expected deep-link to room 1, got: {body}"
    );
}
