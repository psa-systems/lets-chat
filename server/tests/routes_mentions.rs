use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-mentions-tests-{}", std::process::id()));
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

/// Bundle of handles returned by `app_with_two_users` so tests that need to
/// query the `mentions` table directly can do so without re-parsing the
/// router's HTML response.
struct TestApp {
    app: Router,
    session: String,
    viewer_id: String,
    peer_id: String,
    chat: SqlitePool,
}

/// Build a router with `viewer` (admin) and `peer` as members of the seeded
/// General enclave. Returns cloned pool handles - SqlitePool is Arc<Inner>
/// internally so the clone shares state with the AppState pool.
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
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
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

async fn count_mentions_for_user(chat: &SqlitePool, user_id: &str) -> i64 {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM mentions WHERE mentioned_user_id = ?")
        .bind(user_id)
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("n")
}

async fn last_message_id(chat: &SqlitePool, room_id: i64) -> i64 {
    let row = sqlx::query("SELECT id FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1")
        .bind(room_id)
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("id")
}

/// Form-encode a body for application/x-www-form-urlencoded. Test inputs
/// only contain ASCII letters, digits, `@`, spaces, and a `-` so the only
/// transform needed is space->`+`. Anything else would fail the simple
/// substitution and surface as a test bug rather than a silent miscompare.
fn form_encode(body: &str) -> String {
    assert!(
        body.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b' ' | b'-' | b'_')),
        "form_encode helper does not handle char in {body:?}"
    );
    body.replace(' ', "+")
}

async fn post_message(app: &Router, sess: &str, room_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}&file_id=", form_encode(body));
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn patch_message(app: &Router, sess: &str, message_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}", form_encode(body));
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/messages/{message_id}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn autocomplete_returns_room_members() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=al")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("data-username=\"alice\""), "body: {body}");
}

#[tokio::test]
async fn autocomplete_includes_enclave_user_groups() {
    // Per-enclave user groups (LC-83) should show up in the mention popover
    // alongside users so callers do not have to type the entire group name
    // by hand. The group lives in enclave 1, so the General room (id=1)
    // sees it; the row is identified by `data-username="mods"`.
    let t = app_with_two_users("viewer", "alice").await;
    db::user_groups::create(&t.chat, 1, "mods", None, &t.viewer_id)
        .await
        .unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=mo")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("data-username=\"mods\""), "body: {body}");
    let _ = t.peer_id;
}

#[tokio::test]
async fn autocomplete_excludes_self() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=view")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(!body.contains("data-username=\"viewer\""), "body: {body}");
}

#[tokio::test]
async fn autocomplete_lists_broadcast_tokens_first_with_empty_prefix() {
    // Empty prefix: both broadcast tokens appear, and they appear ABOVE the
    // user row (Slack pattern - broadcast tokens are higher-stakes and get
    // the visibility). The general room (id=1) is public; the seeded user
    // "alice" is a member of the General enclave.
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body =
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned();
    let here_at = body.find(r#"data-username="here""#).expect("missing @here");
    let channel_at = body
        .find(r#"data-username="channel""#)
        .expect("missing @channel");
    let alice_at = body
        .find(r#"data-username="alice""#)
        .expect("missing @alice");
    assert!(
        here_at < alice_at && channel_at < alice_at,
        "broadcast tokens did not sort above user row: here@{here_at} channel@{channel_at} alice@{alice_at}"
    );
}

#[tokio::test]
async fn autocomplete_broadcast_prefix_matches_letter_h() {
    // Typing `@h` puts `@here` in the dropdown alongside any matching user
    // ("alice" does not match, but the existence of user rows is not the
    // assertion - the assertion is that @here is visible even though it
    // could compete with `@harry`-style usernames at this prefix).
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=h")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    let body =
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned();
    assert!(
        body.contains(r#"data-username="here""#),
        "@here missing for q=h: {body}"
    );
    assert!(
        !body.contains(r#"data-username="channel""#),
        "@channel should not match prefix h: {body}"
    );
}

#[tokio::test]
async fn autocomplete_broadcast_tokens_absent_in_dm_room() {
    // DMs gate broadcast resolution; surfacing the tokens in the autocomplete
    // would be a confusing no-op. The route must hide them when the room is
    // a DM.
    let t = app_with_two_users("viewer", "alice").await;
    let dm_room = db::chat::create_dm_room(&t.chat, "viewer-alice-dm", &t.viewer_id, &t.peer_id)
        .await
        .unwrap();
    let dm_room_id = dm_room.id;
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/users/mentions?room_id={dm_room_id}&q="))
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body =
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned();
    assert!(
        !body.contains(r#"data-username="here""#),
        "@here leaked into DM autocomplete: {body}"
    );
    assert!(
        !body.contains(r#"data-username="channel""#),
        "@channel leaked into DM autocomplete: {body}"
    );
}

#[tokio::test]
async fn broadcast_count_channel_returns_member_count() {
    // viewer + alice are both in the General enclave; @channel should
    // resolve to 1 (alice; viewer is the author so excluded).
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/rooms/1/broadcast-count?token=channel")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body =
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).into_owned();
    assert!(
        body.contains("1 person in #general"),
        "expected singular '1 person': {body}"
    );
}

#[tokio::test]
async fn broadcast_count_here_with_no_one_online_renders_empty() {
    // No one has opened a WS connection in this test, so @here resolves to
    // zero recipients. The endpoint returns an empty body so the composer
    // slot collapses without a misleading "0 people" line.
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/rooms/1/broadcast-count?token=here")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8_lossy(&to_bytes(resp.into_body(), 1 << 20).await.unwrap())
        .trim()
        .to_string();
    assert!(
        body.is_empty(),
        "expected empty body for 0 recipients: {body:?}"
    );
}

#[tokio::test]
async fn broadcast_count_rejects_unknown_token() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/rooms/1/broadcast-count?token=admins")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn broadcast_count_rejects_dm_room() {
    let t = app_with_two_users("viewer", "alice").await;
    let dm_room = db::chat::create_dm_room(&t.chat, "viewer-alice-dm", &t.viewer_id, &t.peer_id)
        .await
        .unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/api/rooms/{}/broadcast-count?token=channel",
            dm_room.id
        ))
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let resp = t.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn autocomplete_requires_auth() {
    let t = app_with_two_users("viewer", "alice").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/users/mentions?room_id=1&q=al")
        .body(Body::empty())
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
async fn send_message_with_mention_inserts_row() {
    let t = app_with_two_users("viewer", "alice").await;
    // Seeded general room is id 1; both users were added by
    // backfill_general_membership.
    let status = post_message(&t.app, &t.session, 1, "@alice hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let n = count_mentions_for_user(&t.chat, &t.peer_id).await;
    assert_eq!(n, 1, "expected one mention row for alice");
}

#[tokio::test]
async fn edit_removing_mention_deletes_row() {
    let t = app_with_two_users("viewer", "alice").await;
    let status = post_message(&t.app, &t.session, 1, "@alice hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(count_mentions_for_user(&t.chat, &t.peer_id).await, 1);

    let msg_id = last_message_id(&t.chat, 1).await;
    let status = patch_message(&t.app, &t.session, msg_id, "hi alice").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.peer_id).await,
        0,
        "edit to remove @alice should delete the mention row"
    );
}

#[tokio::test]
async fn cross_room_mention_dropped() {
    // Create a fresh private room owned by the viewer admin where the peer
    // is NOT a member. Mentioning @alice in that room should be rejected
    // by the candidate-set filter, leaving the mentions table empty.
    let t = app_with_two_users("viewer", "alice").await;
    let private_room_id = db::chat::create_room(
        &t.chat,
        "secret",
        None,
        "private",
        Some("invite-code-1"),
        None,
    )
    .await
    .unwrap();
    // Only the viewer joins; alice stays out.
    db::chat::add_room_member(&t.chat, private_room_id, &t.viewer_id)
        .await
        .unwrap();

    let status = post_message(&t.app, &t.session, private_room_id, "@alice hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.peer_id).await,
        0,
        "@alice in a private room she isn't in must not insert a mention"
    );
}

#[tokio::test]
async fn self_mention_dropped() {
    let t = app_with_two_users("viewer", "alice").await;
    let status = post_message(&t.app, &t.session, 1, "@viewer hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.viewer_id).await,
        0,
        "self-mention must not insert a mention row"
    );
}
