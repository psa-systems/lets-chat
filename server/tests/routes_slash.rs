//! LC-76 integration: slash commands.
//!
//! Covers /help (ephemeral, not broadcast), /me, /shrug, /remind, unknown
//! fallback to literal, custom static_text, admin-only RBAC, and the
//! autocomplete endpoint.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-slash-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice: String,
    alice_session: String,
    bob_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    for id in [&alice, &bob] {
        sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
            .bind(id)
            .execute(&auth)
            .await
            .unwrap();
    }
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
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
    // Room in an enclave Alice owns; add Bob as a member too.
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    db::enclave::add_member(
        &chat,
        eid,
        &bob,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    TestApp {
        app: routes::build_router(state),
        alice,
        alice_session,
        bob_session,
        chat,
    }
}

async fn make_room(t: &TestApp) -> i64 {
    let eid: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name = 'Acme'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap()
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn post_msg(app: &Router, sess: &str, room: i64, body: &str) -> (StatusCode, String) {
    let form = format!("body={}&file_id=&quote_id=", enc(body));
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn latest_body(chat: &SqlitePool, room: i64) -> Option<String> {
    sqlx::query_scalar("SELECT body FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1")
        .bind(room)
        .fetch_optional(chat)
        .await
        .unwrap()
}

async fn msg_count(chat: &SqlitePool, room: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room)
        .fetch_one(chat)
        .await
        .unwrap()
}

#[tokio::test]
async fn help_is_ephemeral_not_broadcast() {
    let t = app().await;
    let room = make_room(&t).await;
    let (status, body) = post_msg(&t.app, &t.alice_session, room, "/help").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Slash commands"), "help panel rendered");
    assert!(body.contains("/me"), "lists built-ins");
    assert!(
        body.contains("lc-command-result"),
        "ephemeral OOB target, not a broadcast message"
    );
    assert_eq!(msg_count(&t.chat, room).await, 0, "/help posts no message");
}

#[tokio::test]
async fn me_posts_italic_action() {
    let t = app().await;
    let room = make_room(&t).await;
    let (status, _) = post_msg(&t.app, &t.alice_session, room, "/me waves").await;
    // LC-228: slash dispatch returns 204 for message-posting commands (form
    // is `hx-swap="none"`, so the rendered ComposerFragment was discarded).
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(latest_body(&t.chat, room).await.unwrap(), "_waves_");
}

#[tokio::test]
async fn shrug_appends_glyph() {
    let t = app().await;
    let room = make_room(&t).await;
    post_msg(&t.app, &t.alice_session, room, "/shrug oh well").await;
    let body = latest_body(&t.chat, room).await.unwrap();
    assert!(body.starts_with("oh well "));
    assert!(body.contains('\u{30c4}'), "contains the shrug face");
}

#[tokio::test]
async fn unknown_command_falls_back_to_literal() {
    let t = app().await;
    let room = make_room(&t).await;
    post_msg(&t.app, &t.alice_session, room, "/foobar hi there").await;
    assert_eq!(
        latest_body(&t.chat, room).await.unwrap(),
        "/foobar hi there",
        "unknown slash posts the literal text"
    );
}

#[tokio::test]
async fn remind_posts_and_sets_reminder() {
    let t = app().await;
    let room = make_room(&t).await;
    let (status, _) = post_msg(&t.app, &t.alice_session, room, "/remind 1h call mom").await;
    // LC-228: see /me note above.
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(latest_body(&t.chat, room).await.unwrap(), "call mom");
    let pending = db::reminders::list_pending_for_user(&t.chat, &t.alice)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "reminder set on the posted note");
}

#[tokio::test]
async fn custom_static_text_substitutes_args() {
    let t = app().await;
    let room = make_room(&t).await;
    db::slash::insert_global(
        &t.chat,
        "greet",
        "Greet someone",
        db::slash::CustomKind::StaticText,
        "Hello {args}!",
        false,
        "admin",
    )
    .await
    .unwrap();
    post_msg(&t.app, &t.alice_session, room, "/greet world").await;
    assert_eq!(latest_body(&t.chat, room).await.unwrap(), "Hello world!");
}

#[tokio::test]
async fn admin_only_command_rejects_non_admin() {
    let t = app().await;
    let room = make_room(&t).await;
    db::slash::insert_global(
        &t.chat,
        "secret",
        "admins only",
        db::slash::CustomKind::StaticText,
        "the eagle lands at dawn",
        true, // admin_only
        "admin",
    )
    .await
    .unwrap();
    // Bob is a plain user.
    let (status, _) = post_msg(&t.app, &t.bob_session, room, "/secret").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(msg_count(&t.chat, room).await, 0, "no message posted");
}

#[tokio::test]
async fn autocomplete_lists_matching_commands() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/slash-commands?q=he")
        .header(header::COOKIE, format!("session={}", t.alice_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("/help"),
        "autocomplete matches /help for q=he"
    );
}

#[tokio::test]
async fn autocomplete_hides_admin_only_from_non_admin() {
    let t = app().await;
    db::slash::insert_global(
        &t.chat,
        "secretcmd",
        "admins only",
        db::slash::CustomKind::StaticText,
        "x",
        true, // admin_only
        "admin",
    )
    .await
    .unwrap();
    // Bob is a plain user; the admin-only command must not appear.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/slash-commands?q=secret")
        .header(header::COOKIE, format!("session={}", t.bob_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        !body.contains("secretcmd"),
        "admin-only command hidden from non-admin autocomplete"
    );
}
