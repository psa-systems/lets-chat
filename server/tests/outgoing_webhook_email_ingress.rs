//! LC-205: email-ingress posts fire the LC-75 `message.posted` outgoing
//! webhook with the email-inbox actor block.
//!
//! Before LC-205, `finalize_email_inbox_message_send` broadcast to WS and
//! fanned mentions but never enqueued `message.posted`, so every
//! outgoing-webhook subscriber (bridge daemons etc.) silently missed every
//! email-ingress message. This file is the regression guard: it drives the
//! real email-ingress post path (`email_ingress::actor::post_email_message`)
//! and reads the enqueued delivery row, mirroring the LC-78
//! `outgoing_webhook_actor_payload` test shape.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::email_inbox::EmailInboxAuth;
use lets_chat::email_ingress::actor::{post_email_message, PostOutcome};
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [205u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-205-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    app: Router,
    user_id: String,
    room: i64,
    inbox_id: i64,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    // Admin so backfill_general_membership runs; also the negative-case
    // poster (gets a messages:write token below).
    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &user_id)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();

    // Email inbox row for the room.
    let secret_hash = auth::hash_api_token(&SECRET, "inbox-token");
    let inbox_id = db::email_inbox::insert(&chat, room, "Ops Inbox", None, &secret_hash, &user_id)
        .await
        .unwrap();

    // messages:write token for the negative case (user post via the API).
    let token_hash = auth::hash_api_token(&SECRET, "lc205_user");
    db::api_tokens::insert(&auth, &user_id, "tok", &token_hash, "messages:write", None)
        .await
        .unwrap();

    // Global outgoing-webhook subscription so enqueue() writes a delivery
    // row we can inspect (the URL never actually delivers).
    db::outgoing_webhooks::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://127.0.0.1:1/never-delivered",
        "secret",
        &user_id,
    )
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
        secret_key: Some(Arc::new(SECRET)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    Fixture {
        app: routes::build_router(state.clone()),
        state,
        user_id,
        room,
        inbox_id,
    }
}

/// Latest enqueued delivery payload, decoded. Same helper shape as the
/// LC-78 actor-payload test.
async fn latest_payload(chat: &SqlitePool) -> Value {
    let body: String = sqlx::query_scalar(
        "SELECT payload FROM outgoing_webhook_deliveries ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(chat)
    .await
    .unwrap();
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn email_ingress_post_fires_message_posted_with_email_inbox_actor() {
    let fx = setup().await;
    let inbox = EmailInboxAuth {
        id: fx.inbox_id,
        room_id: fx.room,
        name: "Ops Inbox".to_string(),
        avatar_url: None,
        revoked_at: None,
    };
    let outcome = post_email_message(&fx.state, &inbox, "hello from email", &[]).await;
    assert!(
        matches!(outcome, PostOutcome::Posted { .. }),
        "expected Posted, got {outcome:?}"
    );

    let p = latest_payload(&fx.state.chat).await;
    // 1. A message.posted event was enqueued.
    assert_eq!(p["event"], "message.posted");
    // 2. Actor block is the email-inbox variant (specific kind + id), not a user.
    assert_eq!(p["data"]["actor"]["kind"], "email_inbox");
    assert_eq!(p["data"]["actor"]["email_inbox_id"], fx.inbox_id);
    // 3. Payload body matches the email-ingress message body.
    assert_eq!(p["data"]["body"], "hello from email");
    // 4. Payload room_id (enqueue wraps it at the top level) matches the room.
    assert_eq!(p["room_id"], fx.room);
}

#[tokio::test]
async fn non_email_post_in_same_room_uses_user_actor_not_email_inbox() {
    // Anti-over-fire guard: a normal user post in the same room must produce
    // a `user` actor, proving the email-inbox actor is selected by the
    // email PATH, not accidentally stamped on every message.posted.
    let fx = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/rooms/{}/messages", fx.room))
        .header(header::AUTHORIZATION, "Bearer lc205_user")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"body":"hello from a human"}"#.to_string()))
        .unwrap();
    let res = fx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();

    let p = latest_payload(&fx.state.chat).await;
    assert_eq!(p["event"], "message.posted");
    assert_eq!(p["data"]["actor"]["kind"], "user");
    assert_eq!(p["data"]["actor"]["user_id"], fx.user_id);
}
