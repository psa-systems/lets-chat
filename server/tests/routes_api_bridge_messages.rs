//! LC-78: POST /api/v1/bridges/{bridge_id}/messages
//!
//! Covers the bridge-post endpoint's scope isolation (separate from
//! messages:write so the per-message actor override can't be reached by a
//! non-bridge token), the v1 avatar-rejection policy, the bot/bridge
//! ownership gate, and basic input validation.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [9u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-bridge-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    bot_id: String,
    bridge_id: i64,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    // Admin user so backfill_general_membership has someone to seed.
    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    // Bot user that will own the bridge.
    let bot_id = db::auth::create_bot(&auth, "matrixbot").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bot_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "bridged", None, "public", None, Some(eid))
        .await
        .unwrap();
    // Insert the bridge row directly (db::bridges::insert lands in chunk 4).
    let bridge_id: i64 = sqlx::query_scalar(
        "INSERT INTO bridges (room_id, kind, config_encrypted, config_nonce, bot_user_id, created_by) \
         VALUES (?, 'matrix', X'00', X'01', ?, ?) RETURNING id",
    )
    .bind(room)
    .bind(&bot_id)
    .bind(&admin)
    .fetch_one(&chat)
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
        bunyip_sso: None,
        stt_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        bot_id,
        bridge_id,
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

async fn post(app: &Router, token: &str, uri: &str, json: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn bridge_post_happy_path_stores_snapshot() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_ok", "bridge:post").await;
    let (status, body) = post(
        &t.app,
        "lc_br_ok",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"hello from matrix","foreign_name":"alice:matrix.org"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("alice:matrix.org"));
    // Snapshot columns populated on the row, not joined from bridges.
    let row: (Option<i64>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT bridge_id, bridge_foreign_name, bridge_kind FROM messages \
         WHERE body = 'hello from matrix'",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(row.0, Some(t.bridge_id));
    assert_eq!(row.1.as_deref(), Some("alice:matrix.org"));
    assert_eq!(row.2.as_deref(), Some("matrix"));
}

#[tokio::test]
async fn bridge_post_without_bridge_scope_is_403() {
    // A messages:write-only token does NOT reach the bridge endpoint.
    // bridge:post is strictly more powerful (the caller chooses the rendered
    // display name); isolating it to its own scope is the security boundary.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_no", "messages:write").await;
    let (status, _) = post(
        &t.app,
        "lc_br_no",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"x","foreign_name":"alice"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bridge_post_from_wrong_bot_is_403() {
    let t = app().await;
    // A second bot exists but does NOT own this bridge.
    let other = db::auth::create_bot(&t.auth, "otherbot").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&other)
        .execute(&t.auth)
        .await
        .unwrap();
    mint(&t, &other, "lc_br_other", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_br_other",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"impersonator","foreign_name":"alice"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bridge_post_with_invalid_foreign_avatar_url_is_400() {
    // LC-78-AVATAR-PROXY (v2): foreign_avatar is now accepted when the
    // operator gate is on (default), but invalid URLs / non-http(s) schemes
    // / private-resolving hosts are still rejected with 400.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_av_bad", "bridge:post").await;
    let (status, body) = post(
        &t.app,
        "lc_br_av_bad",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"x","foreign_name":"alice","foreign_avatar":"not-a-url"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_lowercase().contains("url"),
        "expected URL-shape error; got {body}"
    );
}

#[tokio::test]
async fn bridge_post_with_private_resolving_foreign_avatar_is_400() {
    // SSRF re-resolve at submit time rejects loopback (`localhost` resolves
    // to 127.0.0.1). The fetch task does its own re-resolve too (LC-152
    // shape), but failing at submit time means the row never lands in the
    // proxy cache for a private target.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_av_loop", "bridge:post").await;
    let (status, body) = post(
        &t.app,
        "lc_br_av_loop",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"x","foreign_name":"alice","foreign_avatar":"http://localhost/avatar.png"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.contains("public"),
        "expected SSRF rejection; got {body}"
    );
}

#[tokio::test]
async fn bridge_post_with_foreign_avatar_stores_hash_v2() {
    // LC-78-AVATAR-PROXY (v2) happy path: accepts a valid public URL, stores
    // the sha256 hash on the message row, upserts a `pending` row in
    // bridge_avatar_proxies. The fetch task is fire-and-forget; this test
    // does NOT wait for it to complete (would require a local HTTP receiver
    // and async timing), only that the wiring is right.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_av_ok", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_br_av_ok",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"hi","foreign_name":"alice","foreign_avatar":"https://example.com/avatar.png"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Hash stored on message row.
    let stored: Option<String> =
        sqlx::query_scalar("SELECT bridge_foreign_avatar FROM messages WHERE body = 'hi'")
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert!(
        stored
            .as_deref()
            .map(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or(false),
        "expected 64-char hex hash on message row; got {stored:?}",
    );
    // Cache row created (pending or ok; the fetch is async so we don't pin status).
    let row_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bridge_avatar_proxies WHERE foreign_url = 'https://example.com/avatar.png'",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(row_count, 1);
}

#[tokio::test]
async fn bridge_post_empty_body_is_400() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_eb", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_br_eb",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"  ","foreign_name":"alice"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bridge_post_empty_foreign_name_is_400() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_ef", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_br_ef",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"hi","foreign_name":"   "}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bridge_post_to_nonexistent_bridge_is_404() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_br_404", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_br_404",
        "/api/v1/bridges/99999/messages",
        r#"{"body":"hi","foreign_name":"alice"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
