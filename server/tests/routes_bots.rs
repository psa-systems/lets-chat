//! LC-73: bot identity. Admin creates a bot (mints a token), the token
//! authenticates the API, the cookie login refuses bots, disabling revokes
//! tokens, and bot-authored messages render with a badge.
//!
//! The admin surface (`/admin/bots`) is standalone-only, so this whole file
//! is gated to the standalone feature.
#![cfg(feature = "standalone")]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [9u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-bots-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    enclave_id: i64,
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
    let enclave_id = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "general", None, "public", None, Some(enclave_id))
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
        enclave_id,
        room,
        auth,
        chat: chat_for_test,
    }
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

async fn api_get(app: &Router, token: &str, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

/// Pull the `lc_...` token out of the create-bot success page.
fn extract_token(html: &str) -> String {
    let start = html.find("lc_").expect("token in page");
    let tail = &html[start..];
    let end = tail
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(tail.len());
    tail[..end].to_string()
}

#[tokio::test]
async fn admin_creates_bot_then_token_authenticates() {
    let t = app().await;
    let (status, body) = post_form(
        &t.app,
        &t.admin_session,
        "/admin/bots",
        "username=helper-bot&s_messages_write=on&s_rooms_read=on",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("helper-bot"));
    let token = extract_token(&body);
    // The minted token authenticates the API.
    assert_eq!(api_get(&t.app, &token, "/api/v1/me").await, StatusCode::OK);
    assert_eq!(
        api_get(&t.app, &token, "/api/v1/rooms").await,
        StatusCode::OK,
        "rooms:read granted"
    );
}

#[tokio::test]
async fn bot_cannot_use_cookie_login() {
    let t = app().await;
    db::auth::create_bot(&t.auth, "login-bot").await.unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("username=login-bot&password=anything"))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    // Login does not succeed: no session cookie is set (success would 303 to /
    // with a Set-Cookie). A rejected login re-renders the form (200) with no
    // session cookie.
    let set_cookie = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().map(|s| s.contains("session=")).unwrap_or(false));
    assert!(!set_cookie, "bot login must not issue a session cookie");
}

#[tokio::test]
async fn disabling_bot_revokes_its_tokens() {
    let t = app().await;
    let (_, body) = post_form(
        &t.app,
        &t.admin_session,
        "/admin/bots",
        "username=tmp-bot&s_messages_write=on",
    )
    .await;
    let token = extract_token(&body);
    assert_eq!(api_get(&t.app, &token, "/api/v1/me").await, StatusCode::OK);

    let bot_id: String = sqlx::query_scalar("SELECT id FROM users WHERE username='tmp-bot'")
        .fetch_one(&t.auth)
        .await
        .unwrap();
    let (status, _) = post_form(
        &t.app,
        &t.admin_session,
        &format!("/admin/bots/{bot_id}/disable"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // Token revoked immediately.
    assert_eq!(
        api_get(&t.app, &token, "/api/v1/me").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn bot_message_renders_with_badge() {
    let t = app().await;
    let (_, body) = post_form(
        &t.app,
        &t.admin_session,
        "/admin/bots",
        "username=poster-bot&s_messages_write=on",
    )
    .await;
    let token = extract_token(&body);
    let bot_id: String = sqlx::query_scalar("SELECT id FROM users WHERE username='poster-bot'")
        .fetch_one(&t.auth)
        .await
        .unwrap();
    // Bot needs enclave membership to access the public room.
    db::enclave::add_member(&t.chat, t.enclave_id, &bot_id, EnclaveRole::Member)
        .await
        .unwrap();

    // Bot posts via the API.
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/rooms/{}/messages", t.room))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"body\":\"beep boop\"}"))
        .unwrap();
    assert_eq!(
        t.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    // Admin opens the room: the bot message carries the badge.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/room/{}", t.room))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("beep boop"), "bot message present");
    assert!(
        html.contains(">bot</span>"),
        "bot badge rendered next to the bot's name"
    );
}
