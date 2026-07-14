//! LC-490 integration: message acknowledgement / required-read tracking.
//!
//! Covers: author flags a message as needing ack; a member acknowledges; the
//! roster count reflects it; clearing the requirement drops the rows; RBAC
//! (a non-author / non-mod cannot flag); and acking a non-required message 400s.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    alice_session: String,
    bob_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    // Promote alice to admin so backfill_general_membership runs and both users
    // get General-enclave access (see CLAUDE.md test-harness note).
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
        alice_session,
        bob_session,
        chat,
    }
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}
async fn post(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    send(app, sess, Method::POST, uri).await
}
async fn del(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    send(app, sess, Method::DELETE, uri).await
}

/// Post a message as the given session and return its id. Goes through the HTTP
/// path so the author id is the real session user (ack RBAC depends on it).
async fn post_message(t: &TestApp, sess: &str, room_id: i64) -> i64 {
    let form = "body=hello%20team&file_id=&quote_id=";
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert!(res.status().is_success() || res.status() == StatusCode::NO_CONTENT);
    sqlx::query_scalar("SELECT id FROM messages ORDER BY id DESC LIMIT 1")
        .fetch_one(&t.chat)
        .await
        .unwrap()
}

#[tokio::test]
async fn require_ack_then_acknowledge_roster() {
    let t = app().await;
    let room_id: i64 = sqlx::query_scalar("SELECT id FROM rooms ORDER BY id LIMIT 1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let mid = post_message(&t, &t.alice_session, room_id).await;

    // Bob (non-author, non-mod) cannot flag it.
    let (status, _) = post(
        &t.app,
        &t.bob_session,
        &format!("/messages/{mid}/ack-required"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-author cannot require ack"
    );

    // Acking before it is required is a 400.
    let (status, _) = post(&t.app, &t.bob_session, &format!("/messages/{mid}/ack")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "cannot ack a non-required message"
    );

    // Alice (author) flags it.
    let (status, _) = post(
        &t.app,
        &t.alice_session,
        &format!("/messages/{mid}/ack-required"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(db::acks::is_required(&t.chat, mid).await.unwrap());

    // Bob acknowledges.
    let (status, body) = post(&t.app, &t.bob_session, &format!("/messages/{mid}/ack")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ack-"), "returns the ack bar fragment");
    let roll = db::acks::rollup(&t.chat, mid, "anyone").await.unwrap();
    assert_eq!(roll.count, 1, "one acknowledgement recorded");

    // Idempotent: acking again keeps count at 1.
    post(&t.app, &t.bob_session, &format!("/messages/{mid}/ack")).await;
    let roll = db::acks::rollup(&t.chat, mid, "anyone").await.unwrap();
    assert_eq!(roll.count, 1, "acknowledge is idempotent");

    // Alice clears the requirement: flag gone, roster gone.
    let (status, _) = del(
        &t.app,
        &t.alice_session,
        &format!("/messages/{mid}/ack-required"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!db::acks::is_required(&t.chat, mid).await.unwrap());
    let roll = db::acks::rollup(&t.chat, mid, "anyone").await.unwrap();
    assert_eq!(roll.count, 0, "clearing the requirement drops the roster");
}
