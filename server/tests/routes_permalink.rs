//! LC-246: message permalink route (`GET /m/{message_id}`).
//!
//! Pins the redirect contract: a room message resolves to
//! `/room/{room_id}#msg-{id}`, a DM message to `/dm/{peer_id}#msg-{id}`, and a
//! missing or inaccessible message returns 404 (never 403 - the permalink must
//! not confirm a message exists in a room the viewer cannot see). Also pins
//! that the hover menu renders the copy-link control on the room page.

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
    // Alice is the admin so backfill_general_membership runs; bob stays a
    // plain user so the inaccessible-room case actually 404s for him.
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
        // secret_key None => the 2FA enrollment middleware is inactive, so
        // authed GETs are not redirected to /settings/2fa/setup.
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

async fn private_room(chat: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'private')")
        .bind(name)
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, Option<String>, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

#[tokio::test]
async fn permalink_to_room_message_redirects_with_fragment() {
    let t = setup().await;
    let mid = insert_message(&t.chat, 1, &t.alice_id, "hello").await;

    let (status, location, _) = get(&t.app, &t.alice_session, &format!("/m/{mid}")).await;
    assert!(status.is_redirection(), "expected a redirect, got {status}");
    assert_eq!(
        location.as_deref(),
        Some(format!("/room/1#msg-{mid}").as_str())
    );
}

#[tokio::test]
async fn permalink_to_dm_message_redirects_to_dm_with_peer() {
    let t = setup().await;
    let dm = db::chat::create_dm_room(&t.chat, "@bob", &t.alice_id, &t.bob_id)
        .await
        .unwrap();
    let mid = insert_message(&t.chat, dm.id, &t.alice_id, "dm hi").await;

    // Alice opening the permalink resolves the peer (bob) for her view.
    let (status, location, _) = get(&t.app, &t.alice_session, &format!("/m/{mid}")).await;
    assert!(status.is_redirection(), "expected a redirect, got {status}");
    assert_eq!(
        location.as_deref(),
        Some(format!("/dm/{}#msg-{mid}", t.bob_id).as_str())
    );
}

#[tokio::test]
async fn permalink_to_missing_message_404s() {
    let t = setup().await;
    let (status, _, _) = get(&t.app, &t.alice_session, "/m/999999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn permalink_to_inaccessible_message_404s_not_403() {
    let t = setup().await;
    // A private room bob is not a member of, with a message by alice.
    let room = private_room(&t.chat, "secret").await;
    let mid = insert_message(&t.chat, room, &t.alice_id, "members only").await;

    // Bob (plain user, not a member) must get 404 - never a 403 that would
    // confirm the message exists.
    let (status, _, _) = get(&t.app, &t.bob_session, &format!("/m/{mid}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn room_page_renders_copy_link_action() {
    let t = setup().await;
    let _ = insert_message(&t.chat, 1, &t.alice_id, "anchor").await;
    let (status, _, body) = get(&t.app, &t.alice_session, "/room/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-copy-link="),
        "hover menu must render the copy-link control",
    );
    assert!(body.contains("Copy link"), "copy-link i18n must resolve");
}
