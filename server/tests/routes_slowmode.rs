//! LC-534 integration: per-channel slowmode enforcement in post_message.
//!
//! Covers the throttle (a member's second rapid post is 429'd), the off case
//! (no throttle when slowmode is 0), and moderator exemption.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::push::MockPushClient;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes, state::AppState};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-slowmode-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    bob_id: String,
    bob_session: String,
    admin_id: String,
    chat: SqlitePool,
}

/// admin (org admin) + bob (plain General member), both able to post in room 1.
async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let general = db::enclave::get_general_id(&chat).await.unwrap().unwrap();
    db::enclave::add_member(&chat, general, &bob_id, EnclaveRole::Member)
        .await
        .unwrap();
    let _ = db::chat::add_room_member(&chat, 1, &bob_id).await;

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        bob_id,
        bob_session,
        admin_id,
        chat,
    }
}

async fn post(app: &Router, sess: &str, body: &str) -> StatusCode {
    let form = format!("body={}&file_id=", body.replace(' ', "+"));
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn slowmode_throttles_a_member() {
    let t = setup().await;
    db::chat::set_room_slowmode(&t.chat, 1, 5).await.unwrap();
    assert!(post(&t.app, &t.bob_session, "first").await.is_success());
    // Immediate second post is inside the 5s cooldown.
    assert_eq!(
        post(&t.app, &t.bob_session, "second").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn off_room_allows_rapid_posts() {
    let t = setup().await;
    // slowmode defaults to 0 (off).
    assert!(post(&t.app, &t.bob_session, "a").await.is_success());
    assert!(post(&t.app, &t.bob_session, "b").await.is_success());
}

#[tokio::test]
async fn moderator_is_exempt() {
    let t = setup().await;
    // Grant bob an explicit room-moderator override.
    db::room_rbac::upsert(&t.chat, 1, &t.bob_id, "moderator", &t.admin_id)
        .await
        .unwrap();
    db::chat::set_room_slowmode(&t.chat, 1, 5).await.unwrap();
    assert!(post(&t.app, &t.bob_session, "one").await.is_success());
    assert!(post(&t.app, &t.bob_session, "two").await.is_success());
}
