//! LC-230: optimistic local echo - server-side contract.
//!
//! The composer tags each send with a client-generated `client_id`; the
//! server echoes it on the author's `ChatEvent::NewMessage` broadcast so the
//! sending tab can dedupe its optimistic placeholder against the canonical
//! WS render. These tests pin that contract:
//!
//!   1. A POST carrying `client_id` broadcasts a NewMessage event carrying
//!      the same id.
//!   2. A POST without `client_id` (every pre-LC-230 client, plus the API /
//!      slash / scheduled paths) broadcasts `client_id: None`.
//!   3. Sanitization drops (never rejects) ids outside `[A-Za-z0-9-]` or
//!      longer than 64 chars - the echo is a UX nicety and must never block
//!      a send.
//!   4. The `NewMessageFragment` OOB wrapper renders `data-lc-client-id`
//!      when (and only when) a client id is attached.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use askama::Template;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::views::message_actor::MessageActor;
use lets_chat::views::room::MessageView;
use lets_chat::views::ws_fragments::NewMessageFragment;
use lets_chat::ws::events::ChatEvent;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use tokio::sync::broadcast::Receiver;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    state: AppState,
    alice_id: String,
    alice_session: String,
    room_id: i64,
}

async fn setup() -> TestApp {
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

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
        secret_key: None,
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: lets_chat::mail::Mailer::from_env(),
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state.clone());
    TestApp {
        app,
        state,
        alice_id,
        alice_session,
        // backfill_general_membership creates the General enclave with room 1.
        room_id: 1,
    }
}

/// POST /room/{id}/messages with a raw url-encoded form body.
async fn post_form(app: &Router, session: &str, room_id: i64, form: &str) -> StatusCode {
    let (status, _) = post_form_with_headers(app, session, room_id, form).await;
    status
}

/// Same as `post_form`, but also returns whether the LC-230 `X-LC-Echo-Drop`
/// response header was set (the no-broadcast-coming signal).
async fn post_form_with_headers(
    app: &Router,
    session: &str,
    room_id: i64,
    form: &str,
) -> (StatusCode, bool) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(form.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let echo_drop = res.headers().contains_key("x-lc-echo-drop");
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, echo_drop)
}

/// Receive the next NewMessage event (skipping unrelated events like typing).
async fn next_new_message(rx: &mut Receiver<ChatEvent>) -> (String, Option<String>) {
    for _ in 0..10 {
        let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("did not receive a hub event within 1s")
            .expect("hub channel closed");
        if let ChatEvent::NewMessage {
            message, client_id, ..
        } = evt
        {
            return (message.body, client_id);
        }
    }
    panic!("no NewMessage event among the first 10 hub events");
}

/// Like `next_new_message`, but returns the full `Message` so callers can feed
/// it back into the per-recipient render.
async fn next_new_message_full(
    rx: &mut Receiver<ChatEvent>,
) -> (lets_chat::models::Message, Option<String>) {
    for _ in 0..10 {
        let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("did not receive a hub event within 1s")
            .expect("hub channel closed");
        if let ChatEvent::NewMessage {
            message, client_id, ..
        } = evt
        {
            return (message, client_id);
        }
    }
    panic!("no NewMessage event among the first 10 hub events");
}

#[tokio::test]
async fn author_gets_echo_even_when_not_subscribed() {
    // LC-397: the author who just sent a message MUST receive their own echo
    // (carrying the client_id, which reconciles the optimistic placeholder)
    // even when this connection is NOT in the room's `subscribed` set. That gap
    // is what left enclave-room sends stuck on "Sending..." while DMs were
    // instant. Render directly with an empty subscription set and assert the
    // echo still carries the client_id.
    let t = setup().await;
    let (_conn, mut rx, _) = t.state.hub.connect(&t.alice_id, "alice");

    let status = post_form(
        &t.app,
        &t.alice_session,
        t.room_id,
        "body=enclave+hi&client_id=cid-xyz",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (message, client_id) = next_new_message_full(&mut rx).await;
    assert_eq!(client_id.as_deref(), Some("cid-xyz"));

    let alice = lets_chat::models::User::from(
        db::auth::find_user_by_id(&t.state.auth, &t.alice_id)
            .await
            .unwrap()
            .unwrap(),
    );
    // This connection has NOT subscribed to the room.
    let not_subscribed: Arc<Mutex<HashSet<i64>>> = Arc::new(Mutex::new(HashSet::new()));

    let out = routes::render_new_message_or_bump(
        &t.state,
        &message,
        client_id.as_deref(),
        &alice,
        &not_subscribed,
    )
    .await;
    let html = out.expect("the author always receives their own echo, even unsubscribed");
    assert!(
        html.contains("data-lc-client-id=\"cid-xyz\""),
        "author echo must carry the client_id so the optimistic placeholder is removed: {html}"
    );
}

#[tokio::test]
async fn post_with_client_id_echoes_it_on_the_broadcast() {
    let t = setup().await;
    let (_conn, mut rx, _) = t.state.hub.connect(&t.alice_id, "alice");

    let status = post_form(
        &t.app,
        &t.alice_session,
        t.room_id,
        "body=hello&client_id=test-cid-123",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (body, client_id) = next_new_message(&mut rx).await;
    assert_eq!(body, "hello");
    assert_eq!(client_id.as_deref(), Some("test-cid-123"));
}

#[tokio::test]
async fn post_without_client_id_broadcasts_none() {
    let t = setup().await;
    let (_conn, mut rx, _) = t.state.hub.connect(&t.alice_id, "alice");

    // Pre-LC-230 form shape: no client_id field at all.
    let status = post_form(&t.app, &t.alice_session, t.room_id, "body=plain").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (body, client_id) = next_new_message(&mut rx).await;
    assert_eq!(body, "plain");
    assert_eq!(client_id, None);
}

#[tokio::test]
async fn invalid_client_id_is_dropped_not_rejected() {
    let t = setup().await;
    let (_conn, mut rx, _) = t.state.hub.connect(&t.alice_id, "alice");

    // Characters outside [A-Za-z0-9-] (url-encoded `<x>!`): send still goes
    // through (204), but the broadcast carries no client id.
    let status = post_form(
        &t.app,
        &t.alice_session,
        t.room_id,
        "body=odd&client_id=%3Cx%3E%21",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (body, client_id) = next_new_message(&mut rx).await;
    assert_eq!(body, "odd");
    assert_eq!(
        client_id, None,
        "non-alphanumeric client_id must be dropped"
    );

    // Oversized (65 chars of 'a'): same drop-not-reject posture.
    let long_id = "a".repeat(65);
    let status = post_form(
        &t.app,
        &t.alice_session,
        t.room_id,
        &format!("body=long&client_id={long_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (body, client_id) = next_new_message(&mut rx).await;
    assert_eq!(body, "long");
    assert_eq!(client_id, None, "oversized client_id must be dropped");

    // Empty string (the composer always submits the field): treated as absent.
    let status = post_form(&t.app, &t.alice_session, t.room_id, "body=blank&client_id=").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (body, client_id) = next_new_message(&mut rx).await;
    assert_eq!(body, "blank");
    assert_eq!(client_id, None, "empty client_id must be treated as absent");
}

#[tokio::test]
async fn quarantined_post_signals_echo_drop_and_broadcasts_nothing() {
    let t = setup().await;
    let (_conn, mut rx, _) = t.state.hub.connect(&t.alice_id, "alice");

    // A link-filter rule with action=quarantine (normal admin-configured
    // deployment state). A matching post returns 204 but never broadcasts,
    // so the composer's optimistic placeholder would otherwise sit as
    // "Sending..." forever - the X-LC-Echo-Drop header tells it to remove
    // the placeholder.
    db::anti_spam::insert_rule(
        &t.state.chat,
        "*.spammy.example",
        db::anti_spam::FilterAction::Quarantine,
        "admin",
    )
    .await
    .unwrap();

    let (status, echo_drop) = post_form_with_headers(
        &t.app,
        &t.alice_session,
        t.room_id,
        "body=see%20https%3A%2F%2Ffoo.spammy.example%2Fx&client_id=quarantine-cid-1",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "quarantine still returns 204"
    );
    assert!(
        echo_drop,
        "quarantine response must carry X-LC-Echo-Drop so the placeholder is removed"
    );

    // No NewMessage broadcast for the quarantined post.
    let got_event = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            match rx.recv().await {
                Ok(ChatEvent::NewMessage { .. }) => break true,
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(!got_event, "quarantined post must not broadcast NewMessage");

    // A normal (non-quarantined) post does NOT carry the header.
    let (status, echo_drop) = post_form_with_headers(
        &t.app,
        &t.alice_session,
        t.room_id,
        "body=clean&client_id=clean-cid-1",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        !echo_drop,
        "a broadcasting post must not carry X-LC-Echo-Drop"
    );
}

fn user_view(body: &str) -> MessageView {
    MessageView {
        id: 1,
        room_id: 1,
        user_id: "00000000-0000-0000-0000-000000000001".to_string(),
        username: "alice".to_string(),
        display_name: None,
        avatar_ext: None,
        status: "active".to_string(),
        custom_status: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        edited_at: None,
        body: body.to_string(),
        reactions: vec![],
        can_edit: true,
        can_delete: true,
        viewer_id: "00000000-0000-0000-0000-000000000001".to_string(),
        seen_caption: None,
        is_follow_up: false,
        show_unread_divider: false,
        day_label: None,
        shame_enabled: false,
        shame_hidden: None,
        reply_count: 0,
        parent_id: None,
        attachments: vec![],
        mentions: vec![],
        is_pinned: false,
        is_bookmarked: false,
        ack: None,
        custom_emojis: vec![],
        quote_preview: None,
        suppress_quote_preview: false,
        is_system: false,
        poll: None,
        follow_up: None,
        author_is_bot: false,
        actor: MessageActor::User,
        channels: vec![],
    }
}

#[tokio::test]
async fn new_message_fragment_renders_dedupe_attribute_only_when_set() {
    let view = user_view("echo me");

    let with_id = NewMessageFragment {
        message: &view,
        client_id: Some("test-cid-456"),
    }
    .render()
    .unwrap();
    assert!(
        with_id.contains(r#"data-lc-client-id="test-cid-456""#),
        "OOB wrapper must carry the dedupe attribute when a client id is attached: {with_id}"
    );

    let without_id = NewMessageFragment {
        message: &view,
        client_id: None,
    }
    .render()
    .unwrap();
    assert!(
        !without_id.contains("data-lc-client-id"),
        "OOB wrapper must stay byte-identical to the pre-LC-230 shape when no client id is attached"
    );
}
