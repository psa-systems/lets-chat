//! LC-250: "mark all as read" (`POST /read-all`).
//!
//! One action clears the viewer's unread message badges (rooms + DMs) and the
//! paired unread mention rows, then returns the re-rendered sidebar.

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
    chat: SqlitePool,
    alice_id: String,
    alice_session: String,
    bob_id: String,
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
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        chat,
        alice_id,
        alice_session,
        bob_id,
    }
}

async fn insert_message(chat: &SqlitePool, room_id: i64, user_id: &str, body: &str) -> i64 {
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn post_read_all(app: &Router, sess: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/read-all")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn read_all_clears_room_dm_unread_and_mentions() {
    let t = setup().await;

    // Two unread room messages from bob in General (room 1).
    let m1 = insert_message(&t.chat, 1, &t.bob_id, "hi 1").await;
    let _m2 = insert_message(&t.chat, 1, &t.bob_id, "hi 2").await;
    // An unread DM message from bob.
    let dm = db::chat::create_dm_room(&t.chat, "@bob", &t.alice_id, &t.bob_id)
        .await
        .unwrap();
    insert_message(&t.chat, dm.id, &t.bob_id, "dm hi").await;
    // An unread mention of alice on m1.
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(m1)
    .bind(1i64)
    .bind(&t.alice_id)
    .bind(&t.bob_id)
    .execute(&t.chat)
    .await
    .unwrap();

    // Sanity: alice starts with unread + a mention.
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, dm.id)
            .await
            .unwrap(),
        1
    );
    assert!(
        !db::mentions::count_unread_mentions_per_room(&t.chat, &t.alice_id)
            .await
            .unwrap()
            .is_empty()
    );

    let (status, body) = post_read_all(&t.app, &t.alice_session).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"sidebar\""),
        "response must be the re-rendered sidebar",
    );

    // Everything cleared.
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap(),
        0,
        "room unread cleared",
    );
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, dm.id)
            .await
            .unwrap(),
        0,
        "DM unread cleared",
    );
    assert!(
        db::mentions::count_unread_mentions_per_room(&t.chat, &t.alice_id)
            .await
            .unwrap()
            .is_empty(),
        "mention chips cleared",
    );
}

#[tokio::test]
async fn read_all_is_idempotent() {
    let t = setup().await;
    insert_message(&t.chat, 1, &t.bob_id, "hi").await;

    let (s1, _) = post_read_all(&t.app, &t.alice_session).await;
    assert_eq!(s1, StatusCode::OK);
    // A second call with nothing unread still succeeds and changes nothing.
    let (s2, _) = post_read_all(&t.app, &t.alice_session).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap(),
        0
    );
}
