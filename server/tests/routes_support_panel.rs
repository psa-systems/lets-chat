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

/// The `before=` cursor on the panel's load-older sentinel, or `None` when the
/// fragment carries no sentinel (i.e. it is the oldest page).
fn older_cursor(html: &str) -> Option<String> {
    const MARKER: &str = "hx-get=\"/support/panel/thread/older?before=";
    let start = html.find(MARKER)? + MARKER.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn bubble_count(html: &str) -> usize {
    html.matches("class=\"lc-support-msg").count()
}

// LC-795: the panel renders one bounded page and can still reach the whole
// conversation. Seeds more than two pages into the support DM, opens the panel,
// follows the sentinel back to the oldest page, and asserts the oldest turn is
// reachable and that the last page stops offering one.
#[tokio::test]
async fn panel_pages_back_to_the_oldest_turn() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    // One send bootstraps the assistant bot and the backing DM; its echo is the
    // oldest message in the room.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=the+very+first+question"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    let dm = db::chat::find_dm_room(&chat, &bot.id, &uid)
        .await
        .unwrap()
        .expect("support DM room created");

    // Seed well past two pages (the panel renders 50 per page), alternating the
    // two participants so both bubble shapes are paged.
    for i in 0..120 {
        let (author, body) = if i % 2 == 0 {
            (&uid, format!("question number {i}"))
        } else {
            (&bot.id, format!("answer number {i}"))
        };
        db::chat::insert_message(&chat, dm.id, author, &body)
            .await
            .unwrap();
    }

    // A fresh panel open renders the NEWEST page: bounded, ending at the last
    // turn, with a sentinel for the history behind it.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let first = body_string(app.clone().oneshot(req).await.unwrap()).await;
    assert!(
        first.contains("answer number 119"),
        "the newest turn is on the first page, got: {first}"
    );
    assert!(
        !first.contains("the very first question"),
        "the oldest turn is NOT on the first page (it would mean an unbounded read), got: {first}"
    );
    assert_eq!(
        bubble_count(&first),
        50,
        "the panel renders exactly one page of bubbles"
    );

    // Follow the sentinel back until a fragment stops offering one. Each hop is
    // the request htmx makes when the sentinel scrolls into view.
    let mut cursor = older_cursor(&first).expect("first page offers a load-older sentinel");
    let mut pages = 1;
    let mut oldest;
    loop {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/support/panel/thread/older?before={cursor}"))
            .header(header::COOKIE, format!("session={session}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        oldest = body_string(res).await;
        pages += 1;
        assert!(
            pages <= 10,
            "paging terminates; a cursor that stops advancing would loop forever"
        );
        // An older page is bubbles only: the stage footer and the welcome block
        // belong to the newest page.
        assert!(
            !oldest.contains("lc-support-stage") && !oldest.contains("lc-support-welcome"),
            "an older page carries bubbles only, got: {oldest}"
        );
        match older_cursor(&oldest) {
            Some(next) => cursor = next,
            None => break,
        }
    }

    assert!(
        pages >= 3,
        "more than two pages were seeded, so more than two were walked, got {pages}"
    );
    assert!(
        oldest.contains("the very first question"),
        "the oldest turn is reachable through the sentinel, got: {oldest}"
    );
    assert!(
        older_cursor(&oldest).is_none(),
        "the last page offers no further sentinel, got: {oldest}"
    );
}

// LC-795: a conversation that fits in one page gets no sentinel at all, so the
// affordance appears when, and only when, there is older history.
#[tokio::test]
async fn short_conversation_renders_no_sentinel() {
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
        .body(Body::from("body=one+short+question"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let thread = body_string(app.clone().oneshot(req).await.unwrap()).await;
    assert!(
        thread.contains("one short question"),
        "the whole conversation is on the page, got: {thread}"
    );
    assert!(
        older_cursor(&thread).is_none(),
        "no sentinel when nothing is older, got: {thread}"
    );

    // The fragment route is gated exactly like the panel: a signed-out caller
    // gets no bubbles.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread/older?before=1")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_ne!(
        res.status(),
        StatusCode::OK,
        "the load-older fragment requires a session"
    );
}

// LC-807: a load-older join that splits a consecutive assistant run re-renders
// the boundary bubble (the row the `before=` cursor names) out of band with no
// avatar, so the joined run shows one avatar, not two. Seeds 60 assistant rows:
// the newest page is the last 50, so the cursor row and the older page's last
// bubble are both the assistant's.
#[tokio::test]
async fn older_page_rerenders_the_boundary_bubble_without_an_avatar() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=the+very+first+question"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    let dm = db::chat::find_dm_room(&chat, &bot.id, &uid)
        .await
        .unwrap()
        .expect("support DM room created");
    for i in 0..60 {
        db::chat::insert_message(&chat, dm.id, &bot.id, &format!("answer number {i}"))
            .await
            .unwrap();
    }

    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let first = body_string(app.clone().oneshot(req).await.unwrap()).await;
    let cursor = older_cursor(&first).expect("first page offers a load-older sentinel");
    // The cursor row is the top of the newest page: an assistant bubble that,
    // as the top of the list, was rendered WITH its avatar.
    let top = format!("<div id=\"lc-support-msg-{cursor}\" class=\"lc-support-msg\">");
    let top_at = first
        .find(&top)
        .expect("the cursor row is the first bubble on the newest page");
    assert!(
        first[top_at..].contains("answer number 10"),
        "seeded 60 assistant rows, so the newest page starts at row 10, got: {first}"
    );
    assert!(
        !first.contains("hx-swap-oob"),
        "the newest page has nothing above it and emits no correction, got: {first}"
    );

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/support/panel/thread/older?before={cursor}"))
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let older = body_string(res).await;
    assert!(
        older.contains("answer number 9"),
        "the older page ends with the assistant row before the cursor, got: {older}"
    );
    let oob = format!(
        "<div id=\"lc-support-msg-{cursor}\" class=\"lc-support-msg\" hx-swap-oob=\"outerHTML\">"
    );
    assert_eq!(
        older.matches(&oob).count(),
        1,
        "the older page carries exactly one OOB copy of the boundary bubble, got: {older}"
    );
    let oob_at = older.find(&oob).unwrap();
    assert!(
        !older[..oob_at].contains("hx-swap-oob"),
        "the page's own bubbles are not out-of-band, got: {older}"
    );
    let corrected = &older[oob_at..];
    assert!(
        corrected.contains("lc-support-avatar-spacer")
            && !corrected.contains("<span class=\"lc-support-avatar\" aria-hidden"),
        "the boundary bubble is re-rendered with the spacer, not the avatar, got: {corrected}"
    );
    assert!(
        corrected.contains("answer number 10"),
        "the OOB copy is the cursor row's own content, got: {corrected}"
    );
}

// LC-807: no correction when the join does not split an assistant run. The
// older page ends with the viewer's own bubble, so the boundary bubble's avatar
// (first of its run) was already right.
#[tokio::test]
async fn older_page_emits_no_correction_after_the_viewers_bubble() {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let (uid, session) = member_session(&auth).await;
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let app: Router = routes::build_router(state(auth.clone(), chat.clone(), settings, true));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/support/panel/send")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("body=the+very+first+question"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let bot = db::auth::find_user_by_username(&auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot created");
    let dm = db::chat::find_dm_room(&chat, &bot.id, &uid)
        .await
        .unwrap()
        .expect("support DM room created");
    // The viewer's row lands just above the newest page's 50 assistant rows.
    db::chat::insert_message(&chat, dm.id, &uid, "one more question")
        .await
        .unwrap();
    for i in 0..50 {
        db::chat::insert_message(&chat, dm.id, &bot.id, &format!("answer number {i}"))
            .await
            .unwrap();
    }

    let req = Request::builder()
        .method(Method::GET)
        .uri("/support/panel/thread")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let first = body_string(app.clone().oneshot(req).await.unwrap()).await;
    let cursor = older_cursor(&first).expect("first page offers a load-older sentinel");
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/support/panel/thread/older?before={cursor}"))
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let older = body_string(res).await;
    assert!(
        older.contains("one more question"),
        "the older page ends with the viewer's bubble, got: {older}"
    );
    assert!(
        !older.contains("hx-swap-oob"),
        "no boundary correction when the join does not split a bot run, got: {older}"
    );
}
