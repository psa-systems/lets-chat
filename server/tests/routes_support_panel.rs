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

// LC-724: after a /human escalation opens a ticket, the panel thread shows the
// "waiting for a human" stage (a live timer + add-details form) keyed on the
// ticket, not the terminal "filed" card. The /human path is synchronous, so the
// stage is visible on the next thread render.
#[tokio::test]
async fn thread_shows_waiting_stage_after_human() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (_uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    let send = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=locked+out&action=human"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(send).await.unwrap().status(),
        StatusCode::OK
    );

    let ticket_id = db::support_tickets::list_open(&chat).await.unwrap()[0].id;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let thread = body_string(app.oneshot(req).await.unwrap()).await;
    assert!(
        thread.contains("lc-support-waiting") && thread.contains("data-lc-support-waiting"),
        "thread shows the waiting stage, got: {thread}"
    );
    assert!(
        thread.contains(&format!("value=\"{ticket_id}\"")),
        "the add-details form targets the opened ticket, got: {thread}"
    );
}

// LC-724: the in-panel "add details" form enriches the still-open ticket with the
// structured context and moves the panel to the terminal "filed" stage.
#[tokio::test]
async fn add_details_enriches_the_ticket_and_files_it() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    // Escalate first so a ticket exists for this user.
    let send = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=locked+out&action=human"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(send).await.unwrap().status(),
        StatusCode::OK
    );
    let ticket_id = db::support_tickets::list_open(&chat).await.unwrap()[0].id;

    // Add details through the form.
    let form = format!(
        "ticket_id={ticket_id}&need=Cannot+sign+in&tried=password+reset&urgency=high&email=me%40example.com"
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/ticket")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(form))
        .unwrap();
    let thread = body_string(app.clone().oneshot(req).await.unwrap()).await;
    assert!(
        thread.contains("lc-support-stage-filed"),
        "the panel moves to the filed stage, got: {thread}"
    );

    // The ticket now carries the richer context (still one ticket, not a second).
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 1);
    let ticket = db::support_tickets::get(&chat, ticket_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        ticket.body.contains("Cannot sign in")
            && ticket.body.contains("What they tried: password reset")
            && ticket.body.contains("Urgency: high")
            && ticket.body.contains("Contact: me@example.com"),
        "ticket enriched with the form context, got: {}",
        ticket.body
    );

    // A confirmation from the bot landed in the user's support DM.
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .unwrap();
    let dm = db::chat::find_dm_room(&chat, &bot.id, &uid)
        .await
        .unwrap()
        .unwrap();
    let last: String =
        sqlx::query_scalar("SELECT body FROM messages WHERE room_id=? ORDER BY id DESC LIMIT 1")
            .bind(dm.id)
            .fetch_one(&chat)
            .await
            .unwrap();
    assert!(
        last.contains("added your details") && last.contains(&format!("#{ticket_id}")),
        "confirmation references the enriched ticket, got: {last}"
    );

    // A stranger cannot enrich someone else's ticket.
    let (_other, other_session) = {
        let id = db::auth::create_user(&auth, "stranger", "h").await.unwrap();
        let s = db::auth::create_session(&auth, &id).await.unwrap();
        (id, s)
    };
    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/ticket")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={other_session}"))
        .body(Body::from(format!("ticket_id={ticket_id}&need=hijack")))
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
    let ticket = db::support_tickets::get(&chat, ticket_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !ticket.body.contains("hijack"),
        "another user cannot overwrite the ticket, got: {}",
        ticket.body
    );
}
