use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::pinned::MAX_PINS_PER_ROOM;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-pinned-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

struct TestApp {
    app: Router,
    viewer_id: String,
    viewer_session: String,
    peer_id: String,
    peer_session: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    // First user becomes admin so the General enclave gets seeded with an
    // owner; without an admin `backfill_general_membership` is a no-op.
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    let peer_session = db::auth::create_session(&auth, &peer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let auth_for_test = auth.clone();
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
        secret_key: None,
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
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
        viewer_id,
        viewer_session,
        peer_id,
        peer_session,
        auth: auth_for_test,
        chat: chat_for_test,
    }
}

async fn seed_dm_room(t: &TestApp) -> i64 {
    db::chat::create_dm_room(&t.chat, "@peer", &t.viewer_id, &t.peer_id)
        .await
        .unwrap()
        .id
}

/// Create a room inside the General enclave (which the viewer is a
/// member of after `backfill_general_membership`).
async fn seed_public_room(t: &TestApp, name: &str) -> i64 {
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, name, None, "public", None, Some(general_id))
        .await
        .unwrap()
}

async fn seed_message(t: &TestApp, room_id: i64, user_id: &str, body: &str) -> i64 {
    db::chat::insert_message(&t.chat, room_id, user_id, body)
        .await
        .unwrap()
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn pin_room_message_returns_oob_strip() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "general-room").await;
    let msg = seed_message(&t, room, &t.peer_id, "important update").await;

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains(&format!(r#"id="lc-pinned-strip-{room}""#)),
        "OOB strip wrapper missing: {body}"
    );
    assert!(
        body.contains(r#"hx-swap-oob="outerHTML""#),
        "OOB swap attribute missing: {body}"
    );
    assert!(
        body.contains("important update"),
        "snippet missing from strip: {body}"
    );
    // Bubble re-render is the acting tab's hover-menu flip. Without it,
    // the user sees "Pin" persist in the menu until they switch rooms.
    assert!(
        body.contains(&format!(r#"id="msg-{msg}""#)),
        "re-rendered bubble missing: {body}"
    );
    assert!(
        body.contains(&format!(r#"hx-delete="/messages/{msg}/pin""#)),
        "bubble's Unpin button missing - hover menu did not flip: {body}"
    );

    // The DB now reflects the pin.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE room_id = ?")
        .bind(room)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

// LC-264: the strip ships the per-room collapse toggle. The fold state is
// JS-persisted (localStorage); this pins that the markup is present.
#[tokio::test]
async fn pinned_strip_ships_collapse_toggle() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "general-room").await;
    let msg = seed_message(&t, room, &t.peer_id, "important update").await;

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("data-lc-pinned-strip"),
        "strip root must carry the collapse marker: {body}"
    );
    assert!(
        body.contains("data-lc-pin-toggle"),
        "strip must ship the collapse toggle: {body}"
    );
    assert!(
        body.contains(&format!(r#"id="lc-pinned-list-{room}""#)),
        "strip must wrap its list in an id'd, collapsible container: {body}"
    );
}

#[tokio::test]
async fn unpin_room_message_returns_oob_strip() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.peer_id, "x").await;
    db::pinned::pin_message(&t.chat, msg, room, &t.viewer_id)
        .await
        .unwrap();

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::DELETE,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"id="lc-pinned-strip-{room}""#)),
        "OOB strip wrapper missing on unpin: {body}"
    );
    // Bubble flips Unpin -> Pin (hx-post) on the unpin reply too.
    assert!(
        body.contains(&format!(r#"id="msg-{msg}""#)),
        "re-rendered bubble missing on unpin: {body}"
    );
    assert!(
        body.contains(&format!(r#"hx-post="/messages/{msg}/pin""#)),
        "bubble's Pin button missing - hover menu did not flip after unpin: {body}"
    );

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE message_id = ?")
        .bind(msg)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn pin_nonexistent_message_returns_404() {
    let t = app_with_two_users("viewer", "peer").await;
    let (status, _) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        "/messages/9999/pin",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pin_soft_deleted_message_returns_404() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "r").await;
    let msg = seed_message(&t, room, &t.peer_id, "x").await;
    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(msg)
        .execute(&t.chat)
        .await
        .unwrap();
    let (status, _) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pin_in_unjoined_private_room_returns_403() {
    let t = app_with_two_users("viewer", "peer").await;
    // Create a private room that the viewer is NOT a member of. The peer
    // is the room's only member.
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let room = db::chat::create_room(
        &t.chat,
        "secret",
        None,
        "private",
        Some("invite-code"),
        Some(general_id),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(room)
        .bind(&t.peer_id)
        .execute(&t.chat)
        .await
        .unwrap();
    let msg = seed_message(&t, room, &t.peer_id, "private content").await;

    // Strip viewer's admin role so god-mode does not apply.
    sqlx::query("UPDATE users SET role='user' WHERE id = ?")
        .bind(&t.viewer_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (status, _) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pin_cap_returns_409_with_message() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "capped").await;
    for _ in 0..MAX_PINS_PER_ROOM {
        let m = seed_message(&t, room, &t.peer_id, "x").await;
        db::pinned::pin_message(&t.chat, m, room, &t.viewer_id)
            .await
            .unwrap();
    }
    let extra = seed_message(&t, room, &t.peer_id, "overflow").await;
    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{extra}/pin"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body.contains("Pin cap reached") && body.contains("50"),
        "expected friendly cap-reached body, got: {body}"
    );
}

#[tokio::test]
async fn dm_either_party_can_pin_and_unpin() {
    let t = app_with_two_users("viewer", "peer").await;
    let dm_id = seed_dm_room(&t).await;
    let msg = seed_message(&t, dm_id, &t.peer_id, "let's meet").await;

    // Viewer pins.
    let (s1, _) = send(
        &t.app,
        &t.viewer_session,
        Method::POST,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(s1, StatusCode::OK);

    // Peer (the OTHER party) unpins, even though they were not the
    // pinner. Confirms the no-per-pinner-restriction rule for DMs.
    let (s2, _) = send(
        &t.app,
        &t.peer_session,
        Method::DELETE,
        &format!("/messages/{msg}/pin"),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE message_id = ?")
        .bind(msg)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn get_room_pins_lists_all_in_newest_first_order() {
    let t = app_with_two_users("viewer", "peer").await;
    let room = seed_public_room(&t, "r").await;
    // Create three messages, pin them in deterministic order with a brief
    // gap so the SQLite second-precision pinned_at differs between rows.
    let m1 = seed_message(&t, room, &t.peer_id, "first body").await;
    db::pinned::pin_message(&t.chat, m1, room, &t.viewer_id)
        .await
        .unwrap();
    sqlx::query("UPDATE pinned_messages SET pinned_at='2026-01-01 00:00:00' WHERE message_id=?")
        .bind(m1)
        .execute(&t.chat)
        .await
        .unwrap();
    let m2 = seed_message(&t, room, &t.peer_id, "second body").await;
    db::pinned::pin_message(&t.chat, m2, room, &t.viewer_id)
        .await
        .unwrap();
    sqlx::query("UPDATE pinned_messages SET pinned_at='2026-01-02 00:00:00' WHERE message_id=?")
        .bind(m2)
        .execute(&t.chat)
        .await
        .unwrap();
    let m3 = seed_message(&t, room, &t.peer_id, "third body").await;
    db::pinned::pin_message(&t.chat, m3, room, &t.viewer_id)
        .await
        .unwrap();
    sqlx::query("UPDATE pinned_messages SET pinned_at='2026-01-03 00:00:00' WHERE message_id=?")
        .bind(m3)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body) = send(
        &t.app,
        &t.viewer_session,
        Method::GET,
        &format!("/room/{room}/pins"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let p1 = body.find("first body").expect("first body present");
    let p2 = body.find("second body").expect("second body present");
    let p3 = body.find("third body").expect("third body present");
    assert!(
        p3 < p2 && p2 < p1,
        "expected newest-first ordering: third < second < first, got {p1}/{p2}/{p3}"
    );
}
