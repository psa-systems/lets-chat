//! LC-77-REPLY commit 1d: end-to-end tests confirming the post path
//! actually invokes the notification dispatcher.
//!
//! The dispatcher itself has gate tests in
//! `email_notification_dispatch.rs`. This file drives the wired-up path:
//! a real `POST /room/{id}/messages` HTTP request that mentions a user,
//! and the DM path. The proof point is the `reply_tokens` row the
//! dispatcher mints when all gates pass; if the row exists, the
//! dispatcher was reached and passed all gates.
//!
//! The fixture uses a real `Mailer` pointing at an unreachable host so
//! the SMTP submit fails (logged), but the reply-token row is inserted
//! BEFORE the send, so the SMTP failure doesn't mask the wiring check.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [31u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-notif-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice_session: String,
    bob_id: String,
    public_room: i64,
    dm_room: i64,
    chat: SqlitePool,
}

async fn setup(bob_opt_in: bool) -> TestApp {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();

    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bob_id)
        .execute(&auth)
        .await
        .unwrap();
    db::auth::set_user_email(&auth, &bob_id, Some("bob@example.com"))
        .await
        .unwrap();
    db::auth::mark_email_verified(&auth, &bob_id, "bob@example.com")
        .await
        .unwrap();
    if bob_opt_in {
        db::auth::set_notify_email_activity_enabled(&auth, &bob_id, true)
            .await
            .unwrap();
    }

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &alice_id)
        .await
        .unwrap();
    // bob is added to the same enclave so an @bob mention in the public
    // room resolves him (the mention resolver gates on room accessibility).
    db::enclave::add_member(
        &chat,
        eid,
        &bob_id,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let public_room = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let dm = db::chat::create_dm_room(&chat, "alice-bob", &alice_id, &bob_id)
        .await
        .unwrap();
    let dm_room = dm.id;

    // LC-363: a mailer pointing at an unreachable host (SMTP submit returns
    // Err, but the dispatcher inserts the reply-token row BEFORE the send).
    // Built without mutating the process-global LETS_CHAT_SMTP_* env vars,
    // which raced across parallel test threads (a concurrent remove_var made
    // this from_env() return None, flipping mail off and failing the assert).
    let mailer = lets_chat::mail::Mailer::unreachable_for_tests();

    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer,
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
        alice_session,
        bob_id,
        public_room,
        dm_room,
        chat: chat_for_test,
    }
}

async fn post_message_via_http(
    app: &Router,
    session: &str,
    room_id: i64,
    body: &str,
) -> StatusCode {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let encoded: String = utf8_percent_encode(body, NON_ALPHANUMERIC).to_string();
    let form = format!("body={encoded}");
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(form))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    status
}

async fn reply_tokens_for_user(pool: &SqlitePool, user_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM reply_tokens WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Wait briefly for the dispatcher's `tokio::spawn` to settle. The post
/// handler returns before SMTP; the dispatcher runs in a spawned task.
async fn wait_for_token(pool: &SqlitePool, user_id: &str, expected: i64) -> i64 {
    for _ in 0..40 {
        let n = reply_tokens_for_user(pool, user_id).await;
        if n >= expected {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    reply_tokens_for_user(pool, user_id).await
}

#[tokio::test]
async fn mention_in_room_message_triggers_dispatcher_and_mints_reply_token() {
    let t = setup(true).await;
    assert_eq!(reply_tokens_for_user(&t.chat, &t.bob_id).await, 0);

    let status = post_message_via_http(&t.app, &t.alice_session, t.public_room, "hey @bob").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let count = wait_for_token(&t.chat, &t.bob_id, 1).await;
    assert_eq!(
        count, 1,
        "an @bob mention in a public room must mint a reply-token row for bob",
    );
}

#[tokio::test]
async fn dm_send_triggers_dispatcher() {
    let t = setup(true).await;
    assert_eq!(reply_tokens_for_user(&t.chat, &t.bob_id).await, 0);

    let status = post_message_via_http(&t.app, &t.alice_session, t.dm_room, "private hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let count = wait_for_token(&t.chat, &t.bob_id, 1).await;
    assert_eq!(
        count, 1,
        "a DM to bob must mint a reply-token row even without an explicit @-mention",
    );
}

#[tokio::test]
async fn opt_out_recipient_dm_does_not_mint_token() {
    let t = setup(false).await;
    assert_eq!(reply_tokens_for_user(&t.chat, &t.bob_id).await, 0);

    let status = post_message_via_http(&t.app, &t.alice_session, t.dm_room, "private hi").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Generous wait; if any reply-token was going to land it would land here.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        reply_tokens_for_user(&t.chat, &t.bob_id).await,
        0,
        "opt-out bob must not receive a reply-token row even from a DM",
    );
}
