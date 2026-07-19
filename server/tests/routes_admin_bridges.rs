//! LC-78: admin bridges UI - registration, listing, plain remove.
//!
//! The `/admin/bridges` surface is standalone-only (mirrors /admin/bots).
//! File-scope feature gate per CLAUDE.md Phase-24 category 3 so saas mode
//! does not compile / run these.
#![cfg(feature = "standalone")]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

/// LC-78: the daemon config blob is opaque to the server. Tests pass a
/// plain-text placeholder so we don't drag in a URL-encoding crate just to
/// embed JSON metacharacters in a form body.
const TEST_CONFIG: &str = "test-daemon-config-blob";

const SECRET: [u8; 32] = [21u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-adbr-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    room: i64,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "bridged", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat,
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
        llm_client: None,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        admin_session,
        room,
        auth,
        chat: chat_for_test,
    }
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_form(app: &Router, sess: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn get_bridges_empty_renders_page() {
    let t = app().await;
    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Bridges"));
    assert!(body.contains("No bridges registered."));
}

// LC-357: a bridge to a nonexistent room is refused inline, and no bot is
// minted (the room check runs before bot creation).
#[tokio::test]
async fn create_bridge_rejects_nonexistent_room() {
    let t = app().await;
    let form = format!("room_id=999999&bot_username=ghostbridge&kind=matrix&config={TEST_CONFIG}");
    let (status, body) = post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    assert_eq!(status, StatusCode::OK, "inline re-render, body: {body}");
    assert!(
        body.contains("does not exist"),
        "expected room-not-found error: {body}"
    );
    let bot: Option<String> =
        sqlx::query_scalar("SELECT username FROM users WHERE username = 'ghostbridge'")
            .fetch_optional(&t.auth)
            .await
            .unwrap();
    assert!(bot.is_none(), "no bot should be created for a bad room");
}

// LC-357: an outgoing webhook scoped to a nonexistent room is refused inline.
#[tokio::test]
async fn create_outgoing_webhook_rejects_nonexistent_scope() {
    let t = app().await;
    let form = "scope_kind=room&scope_id=999999&url=https://example.com/hook&e_message_posted=1";
    let (status, body) =
        post_form(&t.app, &t.admin_session, "/admin/outgoing-webhooks", form).await;
    assert_eq!(status, StatusCode::OK, "inline re-render, body: {body}");
    assert!(
        body.contains("does not exist"),
        "expected scope-not-found error: {body}"
    );
}

#[tokio::test]
async fn create_bridge_creates_bot_token_sealed_config_row() {
    let t = app().await;
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    let (status, body) = post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    // One-time token is shown in the response.
    assert!(
        body.contains("lc_"),
        "expected a one-time token in response"
    );
    // Bot row exists with the bridge role tier.
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE username = 'matrixbridge'")
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert_eq!(role, "bridge");
    // Token minted with exactly the two bridge scopes.
    let scopes: String = sqlx::query_scalar(
        "SELECT scopes FROM api_tokens \
         WHERE user_id = (SELECT id FROM users WHERE username = 'matrixbridge')",
    )
    .fetch_one(&t.auth)
    .await
    .unwrap();
    assert!(scopes.contains("bridge:post"));
    assert!(scopes.contains("bridge:heartbeat"));
    assert!(!scopes.contains("messages:write"));
    // Bridge row inserted with sealed config (BLOB length > 0 means it ran
    // through the AES-GCM seal path, not a placeholder).
    let (kind, enc_len, nonce_len): (String, i64, i64) = sqlx::query_as(
        "SELECT kind, length(config_encrypted), length(config_nonce) FROM bridges \
         WHERE room_id = ?",
    )
    .bind(t.room)
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(kind, "matrix");
    // AES-GCM: ciphertext = plaintext_len + 16-byte tag; nonce is always 12.
    assert!(enc_len >= 16);
    assert_eq!(nonce_len, 12);
}

#[tokio::test]
async fn create_bridge_with_unsupported_kind_is_rejected() {
    // v1 ships matrix only. The schema accepts any TEXT kind (no CHECK) so
    // the gate lives in the handler; this test pins the gate.
    let t = app().await;
    let form = format!(
        "room_id={}&bot_username=ircbot&kind=irc&config={}",
        t.room, TEST_CONFIG
    );
    let (status, body) = post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    assert_eq!(status, StatusCode::OK);
    // Askama escapes single quotes as &#x27;, so match on the unique suffix
    // rather than the literal quoted phrase.
    assert!(
        body.contains("kind is supported in v1"),
        "expected unsupported-kind error in body; got: {}",
        &body[..body.len().min(500)]
    );
    // No bot, no bridge, no token created (rollback verified by absence).
    let bot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'ircbot'")
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert_eq!(bot_count, 0);
}

#[tokio::test]
async fn create_bridge_with_taken_username_shows_error_and_rolls_back() {
    let t = app().await;
    // Pre-create a user with the desired bot username.
    db::auth::create_user(&t.auth, "matrixbridge", "h")
        .await
        .unwrap();
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    let (status, body) = post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("already taken"));
    // No bridge row created.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bridges")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn remove_bridge_preserves_historical_message_snapshot_via_admin_route() {
    let t = app().await;
    // Create a bridge via the admin UI (end-to-end exercise).
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    let bridge_id: i64 = sqlx::query_scalar("SELECT id FROM bridges WHERE kind='matrix'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    // Insert a bridge-authored message directly (full post-flow tested in
    // routes_api_bridge_messages.rs; here we just need the row to exist).
    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, bridge_id, bridge_foreign_name, bridge_kind) \
         VALUES (?, '', 'hello', ?, 'alice:matrix.org', 'matrix')",
    )
    .bind(t.room)
    .bind(bridge_id)
    .execute(&t.chat)
    .await
    .unwrap();
    // Remove via the admin route.
    let (status, _) = post_form(
        &t.app,
        &t.admin_session,
        &format!("/admin/bridges/{}/remove", bridge_id),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER); // Redirect after remove.
                                               // Bridge row gone; snapshot strings persist; bridge_id NULL on the row.
    let bridge_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bridges")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(bridge_count, 0);
    let (msg_bridge_id, msg_name, msg_kind): (Option<i64>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT bridge_id, bridge_foreign_name, bridge_kind FROM messages \
             WHERE body = 'hello'",
        )
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert!(msg_bridge_id.is_none(), "bridge_id nulled on remove");
    assert_eq!(msg_name.as_deref(), Some("alice:matrix.org"));
    assert_eq!(msg_kind.as_deref(), Some("matrix"));
}

#[tokio::test]
async fn list_bridges_shows_pending_status_until_first_heartbeat() {
    let t = app().await;
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    let (status, body) = get(&t.app, &t.admin_session, "/admin/bridges").await;
    assert_eq!(status, StatusCode::OK);
    // The badge text is the derived status (pending until the daemon
    // heartbeats). Match the badge contents directly rather than tags so
    // CSS changes don't break the test.
    assert!(body.contains("pending"), "expected pending status badge");
}

#[tokio::test]
async fn list_bridges_shows_healthy_after_recent_heartbeat() {
    let t = app().await;
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    let bridge_id: i64 = sqlx::query_scalar("SELECT id FROM bridges")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    db::bridges::record_heartbeat(&t.chat, bridge_id, None)
        .await
        .unwrap();
    let (_, body) = get(&t.app, &t.admin_session, "/admin/bridges").await;
    assert!(
        body.contains("healthy"),
        "expected healthy status after heartbeat"
    );
}

#[tokio::test]
async fn list_bridges_shows_stale_when_heartbeat_too_old() {
    let t = app().await;
    let form = format!(
        "room_id={}&bot_username=matrixbridge&kind=matrix&config={}",
        t.room, TEST_CONFIG
    );
    post_form(&t.app, &t.admin_session, "/admin/bridges", &form).await;
    let bridge_id: i64 = sqlx::query_scalar("SELECT id FROM bridges")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    // Force an old heartbeat (10 minutes ago - well beyond the 90s threshold).
    sqlx::query(
        "UPDATE bridges SET last_heartbeat_at = datetime('now', '-10 minutes'), status='healthy' \
         WHERE id = ?",
    )
    .bind(bridge_id)
    .execute(&t.chat)
    .await
    .unwrap();
    let (_, body) = get(&t.app, &t.admin_session, "/admin/bridges").await;
    assert!(body.contains("stale"), "expected stale status badge");
}
