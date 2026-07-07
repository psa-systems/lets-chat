//! LC-548 integration: AI suggested replies (`/messages/{id}/suggest-reply`).
//!
//! Drives the handler over HTTP with a mock LLM: the happy path returns a panel
//! of tappable chips (each an apply button carrying its own draft); a missing
//! message is 404; and with no LLM configured the endpoint is refused (room text
//! never leaves the device).

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
        let p = std::env::temp_dir().join(format!("lc-suggest-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    session: String,
    chat: SqlitePool,
}

async fn app(llm: Option<Arc<dyn lets_chat::llm::LlmClient>>) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let session = db::auth::create_session(&auth, &alice).await.unwrap();
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
        llm_client: llm,
        embedding_client: None,
    };
    db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    TestApp {
        app: routes::build_router(state),
        session,
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

async fn suggest(t: &TestApp, message_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/messages/{message_id}/suggest-reply"))
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn mock(canned: &str) -> Arc<dyn lets_chat::llm::LlmClient> {
    Arc::new(lets_chat::llm::MockLlmClient {
        canned: canned.to_string(),
    })
}

#[tokio::test]
async fn suggest_reply_returns_tappable_chips() {
    let t = app(Some(mock("1. Sounds good, see you then!\n2. On my way."))).await;
    let room = make_room(&t).await;
    let mid = db::chat::insert_message(&t.chat, room, "bob", "running 10 min late, ok?")
        .await
        .unwrap();
    let (status, body) = suggest(&t, mid).await;
    assert_eq!(status, StatusCode::OK);
    // Both drafts render, list markers stripped.
    assert!(
        body.contains("Sounds good, see you then!"),
        "first chip missing: {body}"
    );
    assert!(body.contains("On my way."), "second chip missing: {body}");
    assert!(
        !body.contains("1. Sounds good"),
        "list marker leaked: {body}"
    );
    // Each is an apply button carrying its own suggestion (shared live.js hooks).
    assert!(body.contains("data-lc-ai-apply"));
    assert!(body.matches("data-lc-ai-suggestion").count() >= 2);
}

#[tokio::test]
async fn missing_message_is_404() {
    let t = app(Some(mock("hi"))).await;
    make_room(&t).await;
    let (status, _) = suggest(&t, 999_999).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn no_llm_configured_is_rejected() {
    let t = app(None).await;
    let room = make_room(&t).await;
    let mid = db::chat::insert_message(&t.chat, room, "bob", "hello?")
        .await
        .unwrap();
    // With no LLM, room text must never be forwarded: the endpoint 400s.
    let (status, _) = suggest(&t, mid).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
