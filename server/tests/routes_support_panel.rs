//! LC-718: the support chat bubble HTTP surface (epic LC-717, Phase 1).
//! - The bubble self-gates: empty markup when the assistant is disabled.
//! - Sending a message drives `/support` in the backing bot DM (with a user echo).
//! - The "Talk to a human" action drives `/human` (files a ticket when no admin).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::embeddings::MockEmbeddingClient;
use lets_chat::llm::MockLlmClient;
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes};
use tower::ServiceExt;

mod common;

fn state(
    auth: sqlx::SqlitePool,
    chat: sqlx::SqlitePool,
    settings: sqlx::SqlitePool,
    with_ai: bool,
) -> AppState {
    let bg = lets_chat::bg::spawn(auth.clone());
    AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
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
        llm_client: with_ai.then(|| {
            Arc::new(MockLlmClient {
                canned: "From the docs: try the setup guide.".into(),
            }) as Arc<dyn lets_chat::llm::LlmClient>
        }),
        embedding_client: with_ai.then(|| {
            Arc::new(MockEmbeddingClient::default())
                as Arc<dyn lets_chat::embeddings::EmbeddingClient>
        }),
    }
}

async fn member_session(auth: &sqlx::SqlitePool) -> (String, String) {
    let id = db::auth::create_user(auth, "member", "h").await.unwrap();
    let session = db::auth::create_session(auth, &id).await.unwrap();
    (id, session)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn bubble_is_empty_when_ai_disabled() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (_id, session) = member_session(&auth).await;
    // AI configured but the runtime flag is OFF (never enabled), so the bubble
    // must not render.
    let app: Router = routes::build_router(state(auth, chat, settings, true));

    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/bubble")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        body_string(res).await.trim().is_empty(),
        "bubble renders nothing when the assistant is disabled"
    );
}

#[tokio::test]
async fn bubble_renders_and_send_drives_support() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    // The bubble now renders its launcher + panel.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/bubble")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let bubble = body_string(app.clone().oneshot(req).await.unwrap()).await;
    assert!(
        bubble.contains("lc-support-launcher") && bubble.contains("/support/panel/send"),
        "bubble markup present, got: {bubble}"
    );

    // Sending a plain message runs /support and echoes the user's question.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=how+do+I+reset+my+password"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let thread = body_string(res).await;
    assert!(
        thread.contains("reset my password"),
        "the user's question is echoed into the thread, got: {thread}"
    );

    // A backing assistant-bot DM now holds the conversation.
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    let dm = db::chat::find_dm_room(&chat, &bot.id, &uid)
        .await
        .unwrap()
        .expect("support DM room created");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id=?")
        .bind(dm.id)
        .fetch_one(&chat)
        .await
        .unwrap();
    assert!(count >= 2, "user echo + bot reply persisted, got {count}");
}

#[tokio::test]
async fn talk_to_a_human_files_a_ticket() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (_uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=my+account+is+locked&action=human"))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // With no admin available, /human files a durable ticket (LC-714/716).
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 1);
    let open = db::support_tickets::list_open(&chat).await.unwrap();
    assert!(
        open[0].body.contains("account is locked"),
        "ticket carries the request, got: {}",
        open[0].body
    );
}
