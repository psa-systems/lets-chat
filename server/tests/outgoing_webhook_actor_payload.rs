//! LC-78: outgoing-webhook payload carries an `actor` block on every
//! message-* / reaction.added event so bridge daemons can self-filter
//! "events for messages I produced." This file checks the wiring at each
//! emit site by registering a global outgoing webhook, exercising the
//! producing path, and reading the enqueued delivery row's `payload`.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [19u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-actor-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    user_id: String,
    bot_id: String,
    bridge_id: i64,
    room: i64,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let bot_id = db::auth::create_bot(&auth, "matrixbot").await.unwrap();
    db::auth::set_user_role(&auth, &bot_id, db::auth::ROLE_BRIDGE)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bot_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &user_id)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "bridged", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bridge_id = db::bridges::insert(
        &chat,
        &SECRET,
        room,
        "matrix",
        b"{}",
        &bot_id,
        &user_id,
    )
    .await
    .unwrap();
    // Global outgoing webhook subscribed to every message/reaction event so
    // enqueue() actually writes a delivery row to inspect.
    db::outgoing_webhooks::insert(
        &chat,
        "global",
        None,
        "message.posted message.edited message.deleted reaction.added",
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
    TestApp {
        app: routes::build_router(state),
        user_id,
        bot_id,
        bridge_id,
        room,
        chat,
        auth,
    }
}

async fn mint(t: &TestApp, user_id: &str, plaintext: &str, scopes: &str) {
    let hash = auth::hash_api_token(&SECRET, plaintext);
    db::api_tokens::insert(&t.auth, user_id, "tok", &hash, scopes, None)
        .await
        .unwrap();
}

async fn post_json(app: &Router, token: &str, uri: &str, json: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let s = res.status();
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    s
}

/// Latest enqueued delivery payload, decoded.
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
async fn bridge_post_produces_actor_kind_bridge_with_persistent_id() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_actor_b", "bridge:post").await;
    let status = post_json(
        &t.app,
        "lc_actor_b",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"from matrix","foreign_name":"alice:matrix.org"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let p = latest_payload(&t.chat).await;
    assert_eq!(p["event"], "message.posted");
    assert_eq!(p["data"]["actor"]["kind"], "bridge");
    assert_eq!(p["data"]["actor"]["bridge_id"], t.bridge_id);
}

#[tokio::test]
async fn user_post_produces_actor_kind_user() {
    let t = app().await;
    mint(&t, &t.user_id, "lc_actor_u", "messages:write").await;
    let status = post_json(
        &t.app,
        "lc_actor_u",
        &format!("/api/v1/rooms/{}/messages", t.room),
        r#"{"body":"from alice"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let p = latest_payload(&t.chat).await;
    assert_eq!(p["event"], "message.posted");
    assert_eq!(p["data"]["actor"]["kind"], "user");
    assert_eq!(p["data"]["actor"]["user_id"], t.user_id);
}

#[tokio::test]
async fn webhook_post_produces_actor_kind_webhook() {
    let t = app().await;
    // Create an incoming webhook and post via /webhook/{secret}.
    let secret = lets_chat::auth::generate_api_token();
    let hash = lets_chat::auth::hash_api_token(&SECRET, &secret);
    let webhook_id = db::webhooks::insert(
        &t.chat,
        t.room,
        "alertbot",
        None,
        &hash,
        &t.user_id,
    )
    .await
    .unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/webhook/{}", secret))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"text":"alert!"}"#))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let p = latest_payload(&t.chat).await;
    assert_eq!(p["event"], "message.posted");
    assert_eq!(p["data"]["actor"]["kind"], "webhook");
    assert_eq!(p["data"]["actor"]["webhook_id"], webhook_id);
}

#[tokio::test]
async fn bridge_id_in_actor_block_is_stable_across_calls() {
    // The bridge_id in the actor block is the PERSISTENT bridges.id (chunk-1
    // primary key), NOT a per-session token. A daemon that restarts and
    // re-authenticates with a fresh token continues to filter on the same
    // bridge_id, so the loop-break survives daemon restart. This test posts
    // twice and asserts both payloads carry the same bridge_id.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_actor_stable", "bridge:post").await;
    let uri = format!("/api/v1/bridges/{}/messages", t.bridge_id);
    post_json(
        &t.app,
        "lc_actor_stable",
        &uri,
        r#"{"body":"first","foreign_name":"alice"}"#,
    )
    .await;
    let p1 = latest_payload(&t.chat).await;
    post_json(
        &t.app,
        "lc_actor_stable",
        &uri,
        r#"{"body":"second","foreign_name":"alice"}"#,
    )
    .await;
    let p2 = latest_payload(&t.chat).await;
    assert_eq!(p1["data"]["actor"]["bridge_id"], p2["data"]["actor"]["bridge_id"]);
    assert_eq!(p1["data"]["actor"]["bridge_id"], t.bridge_id);
}
