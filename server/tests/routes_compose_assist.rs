//! LC-532 integration: composer AI writing assistant (`/room/{id}/compose-assist`).
//!
//! Drives the handler over HTTP with a mock LLM: the happy path returns a panel
//! with the suggestion + mode chips; an empty draft is refused; and with no LLM
//! configured the endpoint is refused (the draft never leaves the device).

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
        let p = std::env::temp_dir().join(format!("lc-assist-tests-{}", std::process::id()));
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

async fn assist(t: &TestApp, room: i64, body: &str, action: &str) -> (StatusCode, String) {
    let form = format!("body={body}&action={action}");
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room}/compose-assist"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.session))
        .body(Body::from(form))
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
async fn rephrase_returns_panel_with_suggestion_and_chips() {
    let t = app(Some(mock("Polished version."))).await;
    let room = make_room(&t).await;
    let (status, body) = assist(&t, room, "hey can u fix this", "rephrase").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Polished version."),
        "suggestion missing: {body}"
    );
    assert!(body.contains("data-lc-ai-suggestion"));
    assert!(body.contains("data-lc-ai-apply"));
    // The mode chips render (default locale English label).
    assert!(body.contains("Formal"));
}

#[tokio::test]
async fn empty_draft_is_rejected() {
    let t = app(Some(mock("x"))).await;
    let room = make_room(&t).await;
    let (status, _) = assist(&t, room, "%20%20", "rephrase").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn no_llm_configured_is_rejected() {
    let t = app(None).await;
    let room = make_room(&t).await;
    // With no LLM, the draft must never be forwarded: the endpoint 400s.
    let (status, _) = assist(&t, room, "hello", "rephrase").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
