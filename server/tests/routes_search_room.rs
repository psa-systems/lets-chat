//! LC-268: per-room scoped search (`GET /search?room_id=`).
//!
//! Scopes the existing full-text search to a single room and 403s when the
//! caller cannot access that room.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    chat: SqlitePool,
    alice_id: String,
    bob_id: String,
    alice_session: String,
    bob_session: String,
    enclave_id: i64,
}

async fn setup() -> TestApp {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let enclave_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("id");

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: None,
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        chat,
        alice_id,
        bob_id,
        alice_session,
        bob_session,
        enclave_id,
    }
}

async fn search(app: &Router, sess: &str, room_id: i64, q: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/search?room_id={room_id}&q={q}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

// LC-280: global search with a raw query (operators); spaces percent-encoded.
async fn search_global(app: &Router, sess: &str, q: &str) -> (StatusCode, String) {
    let uri = format!("/search?q={}", q.replace(' ', "%20"));
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn room_scoped_search_excludes_other_rooms() {
    let t = setup().await;
    let room_a = db::chat::create_room(&t.chat, "alpha", None, "public", None, Some(t.enclave_id))
        .await
        .unwrap();
    let room_b = db::chat::create_room(&t.chat, "beta", None, "public", None, Some(t.enclave_id))
        .await
        .unwrap();
    db::chat::insert_message(&t.chat, room_a, "alice", "needle apple")
        .await
        .unwrap();
    db::chat::insert_message(&t.chat, room_b, "alice", "needle banana")
        .await
        .unwrap();

    let (status, body) = search(&t.app, &t.alice_session, room_a, "needle").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("apple"), "room A's hit must appear: {body}");
    assert!(
        !body.contains("banana"),
        "room B's hit must NOT appear in a room-A-scoped search: {body}"
    );
}

#[tokio::test]
async fn room_search_forbidden_when_inaccessible() {
    let t = setup().await;
    let secret =
        db::chat::create_room(&t.chat, "secret", None, "private", None, Some(t.enclave_id))
            .await
            .unwrap();
    db::chat::insert_message(&t.chat, secret, "alice", "needle hidden")
        .await
        .unwrap();

    // bob is a non-admin and not a member of the private room.
    let (status, _) = search(&t.app, &t.bob_session, secret, "needle").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-member cannot search a private room they cannot access",
    );
}

// LC-280: from:<username> narrows results to that author; an unknown user
// matches nothing.
#[tokio::test]
async fn from_operator_filters_by_author() {
    let t = setup().await;
    let room = db::chat::create_room(&t.chat, "ops", None, "public", None, Some(t.enclave_id))
        .await
        .unwrap();
    db::chat::insert_message(&t.chat, room, &t.alice_id, "needle from alice")
        .await
        .unwrap();
    db::chat::insert_message(&t.chat, room, &t.bob_id, "needle from bob")
        .await
        .unwrap();

    // alice is admin (sees all rooms). Filter to bob's authorship.
    let (status, body) = search_global(&t.app, &t.alice_session, "from:bob needle").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("needle from bob"),
        "bob's hit must appear: {body}"
    );
    assert!(
        !body.contains("needle from alice"),
        "from:bob must exclude alice's hit: {body}"
    );
}

#[tokio::test]
async fn from_operator_unknown_user_returns_empty() {
    let t = setup().await;
    let room = db::chat::create_room(&t.chat, "ops2", None, "public", None, Some(t.enclave_id))
        .await
        .unwrap();
    db::chat::insert_message(&t.chat, room, &t.alice_id, "needle here")
        .await
        .unwrap();

    let (status, body) = search_global(&t.app, &t.alice_session, "from:nobody needle").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("role=\"listbox\""),
        "an unknown from: user yields no results: {body}"
    );
}
