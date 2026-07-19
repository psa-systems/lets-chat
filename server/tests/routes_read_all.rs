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
    bob_session: String,
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
    // LC-258: bob stays a non-admin (no role bump) so the access-gate test can
    // assert a 403 on a private room he is not a member of.
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
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
        embedding_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        chat,
        alice_id,
        alice_session,
        bob_id,
        bob_session,
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

// LC-258: per-room mark-as-read.
async fn post_room_read(app: &Router, sess: &str, room_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/read"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

// LC-286: mark a conversation unread from a message.
async fn post_message_unread(app: &Router, sess: &str, message_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/messages/{message_id}/unread"))
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
        !db::mentions::count_unread_mentions_per_room(&t.chat, &t.alice_id, false)
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
        db::mentions::count_unread_mentions_per_room(&t.chat, &t.alice_id, false)
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

// LC-258: marking ONE room read clears only that room (and its mentions),
// leaving every other unread conversation untouched.
#[tokio::test]
async fn read_one_room_clears_only_that_room() {
    let t = setup().await;

    // Unread in room 1 (General) + a mention of alice there.
    let m1 = insert_message(&t.chat, 1, &t.bob_id, "hi 1").await;
    insert_message(&t.chat, 1, &t.bob_id, "hi 2").await;
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
    // Unread in a separate DM that must NOT be cleared.
    let dm = db::chat::create_dm_room(&t.chat, "@bob", &t.alice_id, &t.bob_id)
        .await
        .unwrap();
    insert_message(&t.chat, dm.id, &t.bob_id, "dm hi").await;

    let (status, body) = post_room_read(&t.app, &t.alice_session, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"sidebar\""),
        "returns the sidebar fragment"
    );

    // Room 1 + its mention cleared.
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap(),
        0,
        "target room unread cleared",
    );
    assert!(
        db::mentions::count_unread_mentions_per_room(&t.chat, &t.alice_id, false)
            .await
            .unwrap()
            .is_empty(),
        "target room mention cleared",
    );
    // The DM is untouched.
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, dm.id)
            .await
            .unwrap(),
        1,
        "other conversation stays unread",
    );
}

// LC-258: a viewer cannot mark a room they cannot see. bob (non-admin) is not a
// member of a fresh private room, so the access gate returns 403.
#[tokio::test]
async fn read_room_forbidden_when_inaccessible() {
    let t = setup().await;
    let private = sqlx::query("INSERT INTO rooms (name, room_type) VALUES ('secret', 'private')")
        .execute(&t.chat)
        .await
        .unwrap()
        .last_insert_rowid();

    let (status, _) = post_room_read(&t.app, &t.bob_session, private).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-member cannot mark an inaccessible private room read",
    );
}

// LC-286: marking a message unread rewinds the watermark so it (and newer)
// re-raise the unread badge.
#[tokio::test]
async fn mark_unread_rewinds_the_watermark() {
    let t = setup().await;
    insert_message(&t.chat, 1, &t.bob_id, "first").await;
    let m2 = insert_message(&t.chat, 1, &t.bob_id, "second").await;
    // Read up to the latest: unread is 0.
    db::chat::set_last_read(&t.chat, &t.alice_id, 1, m2)
        .await
        .unwrap();
    assert_eq!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap(),
        0
    );

    let (status, body) = post_message_unread(&t.app, &t.alice_session, m2).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"sidebar\""),
        "returns the sidebar fragment"
    );
    assert!(
        db::chat::get_unread_count(&t.chat, &t.alice_id, 1)
            .await
            .unwrap()
            >= 1,
        "the message and newer become unread again",
    );
}

#[tokio::test]
async fn mark_unread_forbidden_when_inaccessible() {
    let t = setup().await;
    let private = sqlx::query("INSERT INTO rooms (name, room_type) VALUES ('secret', 'private')")
        .execute(&t.chat)
        .await
        .unwrap()
        .last_insert_rowid();
    let msg = insert_message(&t.chat, private, &t.alice_id, "hidden").await;

    // bob (non-admin) is not a member of the private room.
    let (status, _) = post_message_unread(&t.app, &t.bob_session, msg).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn mark_unread_not_found_for_missing_message() {
    let t = setup().await;
    let (status, _) = post_message_unread(&t.app, &t.alice_session, 999_999).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
