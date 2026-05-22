use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::notifications::MuteMode;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-mute-tests-{}", std::process::id()));
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
    session: String,
    viewer_id: String,
    peer_id: String,
    chat: SqlitePool,
}

/// Build a router with `viewer` (admin) and `peer` as members of the seeded
/// General enclave. The seeded `general` room has id 1.
async fn app_with_two_users(viewer: &str, peer: &str) -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, viewer, "hash").await.unwrap();
    let peer_id = db::auth::create_user(&auth, peer, "hash").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
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
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        session,
        viewer_id,
        peer_id,
        chat: chat_for_test,
    }
}

async fn count_mute_rows(chat: &SqlitePool, user_id: &str, room_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_one(chat)
    .await
    .unwrap()
}

async fn post_notify_prefs(app: &Router, sess: &str, room_id: i64, mode: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/notify-prefs"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(format!("mute_mode={mode}")))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn post_notify_prefs_full(
    app: &Router,
    sess: &str,
    room_id: i64,
    mode: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/notify-prefs"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(format!("mute_mode={mode}")))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_room_page(app: &Router, sess: &str, room_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/room/{room_id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn post_persists_mute_mode_and_returns_swapped_header() {
    let t = app_with_two_users("viewer", "alice").await;
    let (status, body) = post_notify_prefs_full(&t.app, &t.session, 1, "all").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"id="lc-room-header""#),
        "header swap target missing: {body}"
    );
    assert!(
        body.contains(r#"value="all" checked"#),
        "expected 'all' radio checked: {body}"
    );
    assert!(
        body.contains(">Muted"),
        "expected Muted button label: {body}"
    );

    let stored: String = sqlx::query_scalar(
        "SELECT mute_mode FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind(&t.viewer_id)
    .bind(1_i64)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(stored, "all");
}

#[tokio::test]
async fn post_with_none_deletes_row() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "all").await,
        StatusCode::OK
    );
    assert_eq!(count_mute_rows(&t.chat, &t.viewer_id, 1).await, 1);
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "none").await,
        StatusCode::OK
    );
    assert_eq!(count_mute_rows(&t.chat, &t.viewer_id, 1).await, 0);
}

#[tokio::test]
async fn post_invalid_mode_returns_400() {
    let t = app_with_two_users("viewer", "alice").await;
    let status = post_notify_prefs(&t.app, &t.session, 1, "bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_unauthenticated_redirects_or_unauthorized() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/notify-prefs")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("mute_mode=all"))
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::SEE_OTHER
            || resp.status() == StatusCode::TEMPORARY_REDIRECT
            || resp.status() == StatusCode::FOUND
            || resp.status() == StatusCode::UNAUTHORIZED,
        "status: {}",
        resp.status()
    );
}

#[tokio::test]
async fn post_to_dm_room_returns_400() {
    let t = app_with_two_users("viewer", "alice").await;
    // Create a DM room. The viewer is admin so any user pair will do.
    let dm = db::chat::create_dm_room(&t.chat, "@alice", &t.viewer_id, &t.peer_id)
        .await
        .unwrap();
    let status = post_notify_prefs(&t.app, &t.session, dm.id, "all").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_to_inaccessible_private_room_returns_403() {
    // Create a fresh user (non-admin) with no membership in a private room
    // and POST as them.
    let t = app_with_two_users("viewer", "alice").await;
    // Promote alice to non-admin and login as her.
    sqlx::query("UPDATE users SET role='user' WHERE id=?")
        .bind(&t.peer_id)
        .execute(&t.chat) // wrong pool, but we need auth pool
        .await
        .ok();
    // Use the actual auth pool path: re-derive via a session for the peer.
    // Build a private room owned by viewer; alice is not a member.
    let private_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) \
         SELECT 'private-1', 'private', enclave_id FROM rooms WHERE id = 1 \
         RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query("INSERT INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(private_id)
        .bind(&t.viewer_id)
        .execute(&t.chat)
        .await
        .unwrap();

    // Get a non-admin auth and session for `alice`.
    let auth_pool = open_pool("auth").await;
    let chat_pool = t.chat.clone();
    let alice_id = db::auth::create_user(&auth_pool, "alice2", "hash")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='user', totp_enabled=1 WHERE id=?")
        .bind(&alice_id)
        .execute(&auth_pool)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth_pool, &alice_id)
        .await
        .unwrap();
    let settings = open_pool("settings").await;
    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
        chat: chat_pool,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    let status = post_notify_prefs(&app, &alice_session, private_id, "all").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sidebar_renders_muted_room_with_greyed_class() {
    let t = app_with_two_users("viewer", "alice").await;
    // The viewer is the active room, so its sidebar entry has
    // `aria-current="page"` and the active-room style would mask
    // `text-slate-400`. Use a second room: create it, mute it, then GET
    // room 1 and confirm the second room's sidebar link renders greyed.
    let other_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) \
         SELECT 'other', 'public', enclave_id FROM rooms WHERE id = 1 \
         RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, other_id, "all").await,
        StatusCode::OK
    );
    let (status, body2) = get_room_page(&t.app, &t.session, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body2.contains(&format!("/room/{other_id}")),
        "second room link missing: {body2}"
    );
    // Find the anchor for the muted room and verify it carries text-slate-400.
    let anchor_marker = format!(r#"href="/room/{other_id}""#);
    let idx = body2.find(&anchor_marker).expect("muted room anchor");
    let rest = &body2[idx..];
    let close = rest.find('>').expect("anchor close");
    let opening = &rest[..close];
    assert!(
        opening.contains("text-slate-400"),
        "expected greyed class on muted room anchor: {opening}"
    );
}

#[tokio::test]
async fn sidebar_unread_badge_hidden_for_muted_room() {
    let t = app_with_two_users("viewer", "alice").await;
    // Create a second public room so the muted one isn't the active room
    // (active rooms have unread = 0 anyway after mark-as-read).
    let other_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) \
         SELECT 'other', 'public', enclave_id FROM rooms WHERE id = 1 \
         RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    // Have alice post messages in `other` so the viewer accumulates unread.
    for i in 0..3 {
        sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
            .bind(other_id)
            .bind(&t.peer_id)
            .bind(format!("hello {i}"))
            .execute(&t.chat)
            .await
            .unwrap();
    }
    // Without muting, the badge would render `>3<`.
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, other_id, "all").await,
        StatusCode::OK
    );
    // GET room 1 (which renders the sidebar). The muted room's unread span
    // should be empty, not show a count chip.
    let (status, body) = get_room_page(&t.app, &t.session, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"<span id="unread-room-{other_id}"></span>"#)),
        "expected empty unread span for muted room {other_id}, got body: {body}"
    );
}

#[tokio::test]
async fn mention_badge_still_renders_in_except_mentions_mode() {
    let t = app_with_two_users("viewer", "alice").await;
    // Create a second public room so room 1 is the active (open) room and
    // `other` is the muted-with-mentions room we're testing.
    let other_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) \
         SELECT 'other', 'public', enclave_id FROM rooms WHERE id = 1 \
         RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    // Insert a message authored by alice and a mention row for the viewer.
    let msg_id: i64 = sqlx::query_scalar(
        "INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, '@viewer hi') RETURNING id",
    )
    .bind(other_id)
    .bind(&t.peer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(other_id)
    .bind(&t.viewer_id)
    .bind(&t.peer_id)
    .execute(&t.chat)
    .await
    .unwrap();
    // Mute `other` with except_mentions.
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, other_id, "except_mentions").await,
        StatusCode::OK
    );
    let (status, body) = get_room_page(&t.app, &t.session, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(
            r#"<span id="mention-room-{other_id}" class="ml-1 text-[10px] font-bold uppercase bg-red-600 text-white rounded px-1.5">@1</span>"#
        )),
        "expected mention badge for except_mentions room, got body: {body}"
    );
}

#[tokio::test]
async fn mark_mentions_read_still_runs_for_muted_room_open() {
    let t = app_with_two_users("viewer", "alice").await;
    let msg_id: i64 = sqlx::query_scalar(
        "INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, '@viewer hi') RETURNING id",
    )
    .bind(1_i64)
    .bind(&t.peer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(1_i64)
    .bind(&t.viewer_id)
    .bind(&t.peer_id)
    .execute(&t.chat)
    .await
    .unwrap();
    // Mute room 1 entirely.
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "all").await,
        StatusCode::OK
    );
    // Verify mention is unread.
    let read_at_before: Option<String> = sqlx::query_scalar(
        "SELECT read_at FROM mentions WHERE message_id = ? AND mentioned_user_id = ?",
    )
    .bind(msg_id)
    .bind(&t.viewer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert!(read_at_before.is_none());
    // Open room 1; the page-render path should still mark the mention read.
    let (status, _body) = get_room_page(&t.app, &t.session, 1).await;
    assert_eq!(status, StatusCode::OK);
    let read_at_after: Option<String> = sqlx::query_scalar(
        "SELECT read_at FROM mentions WHERE message_id = ? AND mentioned_user_id = ?",
    )
    .bind(msg_id)
    .bind(&t.viewer_id)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert!(
        read_at_after.is_some(),
        "muted room open did not advance mention read_at"
    );
}

#[tokio::test]
async fn room_page_button_label_reflects_persisted_mode() {
    let t = app_with_two_users("viewer", "alice").await;
    // Default: button shows "Unmuted".
    let (status, body) = get_room_page(&t.app, &t.session, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"id="lc-notify-toggle-1""#));
    assert!(
        body.contains(">Unmuted"),
        "default label wrong: snippet missing"
    );
    // Mute and reload: button shows "Muted (mentions on)".
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "except_mentions").await,
        StatusCode::OK
    );
    let (_, body) = get_room_page(&t.app, &t.session, 1).await;
    assert!(
        body.contains(">Muted (mentions on)"),
        "except_mentions label wrong: {body}"
    );
}

#[tokio::test]
async fn dm_page_does_not_render_notify_dropdown() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/dm/{}", t.peer_id))
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        !body.contains("lc-notify-toggle-"),
        "DM page should not render the notify dropdown: {body}"
    );
}

#[tokio::test]
async fn upserting_mute_mode_overwrites_prior() {
    let t = app_with_two_users("viewer", "alice").await;
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "all").await,
        StatusCode::OK
    );
    assert_eq!(
        post_notify_prefs(&t.app, &t.session, 1, "except_mentions").await,
        StatusCode::OK
    );
    let row = sqlx::query(
        "SELECT mute_mode FROM room_notification_settings WHERE user_id = ? AND room_id = ?",
    )
    .bind(&t.viewer_id)
    .bind(1_i64)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("mute_mode"), "except_mentions");
}

// MuteMode predicate tests guard the WS-arm logic. The WS arms call
// `MuteMode::allows_room_mention` and `MuteMode::allows_unread_bump`
// directly; verifying the predicate matrix here ensures the WS render
// behavior is correct by construction.
#[test]
fn allows_room_mention_matrix() {
    assert!(MuteMode::None.allows_room_mention());
    assert!(MuteMode::ExceptMentions.allows_room_mention());
    assert!(!MuteMode::All.allows_room_mention());
}

#[test]
fn allows_unread_bump_matrix() {
    assert!(MuteMode::None.allows_unread_bump());
    assert!(!MuteMode::ExceptMentions.allows_unread_bump());
    assert!(!MuteMode::All.allows_unread_bump());
}

#[tokio::test]
async fn private_room_page_renders_notify_dropdown() {
    // LC-90 AC: the all/mentions/none toggle must be reachable from the room
    // header for every room kind that supports it. The seeded general room
    // (id 1) covers the public + enclave-scoped case; this guards a
    // standalone private channel rendering the same shared dropdown.
    let t = app_with_two_users("viewer", "alice").await;
    let room_id = db::chat::create_room(&t.chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&t.chat, room_id, &t.viewer_id)
        .await
        .unwrap();
    let (status, body) = get_room_page(&t.app, &t.session, room_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"id="lc-notify-toggle-{room_id}""#)),
        "private room page must render the notify dropdown: {body}"
    );
}
