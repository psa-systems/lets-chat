//! LC-492 integration: in-channel AI assistant (`/ask`).
//!
//! Covers gating (disabled room is refused, nothing posted) and the happy path
//! (enabled room posts the LLM answer as the `assistant` bot) with a mock LLM.

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
        let p = std::env::temp_dir().join(format!("lc-assistant-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice_session: String,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let mock: Arc<dyn lets_chat::llm::LlmClient> = Arc::new(lets_chat::llm::MockLlmClient {
        canned: "The answer is 42.".to_string(),
    });
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
        llm_client: Some(mock),
        embedding_client: None,
    };
    db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    TestApp {
        app: routes::build_router(state),
        alice_session,
        chat,
        auth,
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

async fn post_msg(app: &Router, sess: &str, room: i64, body: &str) -> StatusCode {
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
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    status
}

async fn msg_count(chat: &SqlitePool, room: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room)
        .fetch_one(chat)
        .await
        .unwrap()
}

#[tokio::test]
async fn ask_refused_when_room_assistant_disabled() {
    let t = app().await;
    let room = make_room(&t).await;
    // Default off: /ask is refused and nothing is posted.
    let status = post_msg(&t.app, &t.alice_session, room, "/ask what is the answer").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(msg_count(&t.chat, room).await, 0);
    // The assistant bot is not created by a refused ask.
    assert!(db::auth::find_user_by_username(&t.auth, "assistant")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn ask_posts_answer_as_assistant_bot_when_enabled() {
    let t = app().await;
    let room = make_room(&t).await;
    db::chat::set_room_assistant_enabled(&t.chat, room, true)
        .await
        .unwrap();

    let status = post_msg(&t.app, &t.alice_session, room, "/ask what is the answer").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Exactly one message: the assistant's answer (the /ask itself is not echoed).
    assert_eq!(msg_count(&t.chat, room).await, 1);
    let (body, author): (String, String) = sqlx::query_as(
        "SELECT body, user_id FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(room)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert!(body.contains("The answer is 42."), "carries the LLM answer");
    assert!(
        body.contains("asked:") && body.contains("what is the answer"),
        "quotes the asker's question"
    );

    // The author is the assistant bot.
    let bot = db::auth::find_user_by_username(&t.auth, "assistant")
        .await
        .unwrap()
        .expect("bot created");
    assert_eq!(author, bot.id);
    assert!(bot.is_bot);
}
