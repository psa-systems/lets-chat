//! LC-74: incoming webhooks. Admin creates a webhook (URL revealed once),
//! POSTing to it appends a webhook-attributed message, revoke -> 410, unknown
//! secret -> 401, rate limit -> 429, markdown flag, and room-delete cascade.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [5u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-wh-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    room: i64,
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
    let room = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
    let state = AppState {
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
    };
    TestApp {
        app: routes::build_router(state),
        admin_session,
        room,
        chat: chat_for_test,
    }
}

async fn create_webhook(t: &TestApp, name: &str) -> String {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{}/webhooks", t.room))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(format!("name={name}&avatar_url=")))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    // Extract the secret from the revealed "<base>/webhook/<secret>" URL.
    let pos = html.find("/webhook/").expect("webhook url shown");
    let tail = &html[pos + "/webhook/".len()..];
    let end = tail
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(tail.len());
    tail[..end].to_string()
}

async fn post_webhook(app: &Router, secret: &str, json: &str) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/webhook/{secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let retry = res
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    (status, retry)
}

async fn latest(chat: &SqlitePool, room: i64) -> (String, String, Option<i64>) {
    let row = sqlx::query(
        "SELECT user_id, body, webhook_id FROM messages WHERE room_id=? ORDER BY id DESC LIMIT 1",
    )
    .bind(room)
    .fetch_one(chat)
    .await
    .unwrap();
    use sqlx::Row;
    (
        row.get::<String, _>("user_id"),
        row.get::<String, _>("body"),
        row.get::<Option<i64>, _>("webhook_id"),
    )
}

#[tokio::test]
async fn create_post_and_attribute_to_webhook() {
    let t = app().await;
    let secret = create_webhook(&t, "Grafana").await;
    let (status, _) = post_webhook(&t.app, &secret, "{\"text\":\"alert fired\"}").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (user_id, body, webhook_id) = latest(&t.chat, t.room).await;
    assert_eq!(user_id, "", "webhook message has no user");
    assert!(webhook_id.is_some(), "attributed to a webhook");
    assert_eq!(body, "alert fired");

    // Room page renders the webhook name + badge.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/room/{}", t.room))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("Grafana"));
    assert!(html.contains(">webhook</span>"), "webhook badge rendered");
}

#[tokio::test]
async fn oversized_text_is_400() {
    let t = app().await;
    let secret = create_webhook(&t, "Big").await;
    let huge = "x".repeat(17 * 1024);
    let json = format!("{{\"text\":\"{huge}\"}}");
    let (status, _) = post_webhook(&t.app, &secret, &json).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_secret_is_401() {
    let t = app().await;
    let (status, _) = post_webhook(&t.app, "lc_nope", "{\"text\":\"x\"}").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_webhook_is_410() {
    let t = app().await;
    let secret = create_webhook(&t, "Old").await;
    assert_eq!(
        post_webhook(&t.app, &secret, "{\"text\":\"a\"}").await.0,
        StatusCode::NO_CONTENT
    );
    let wid: i64 = sqlx::query_scalar("SELECT id FROM incoming_webhooks WHERE room_id=?")
        .bind(t.room)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{}/webhooks/{}/revoke", t.room, wid))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        t.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_webhook(&t.app, &secret, "{\"text\":\"b\"}").await.0,
        StatusCode::GONE
    );
}

#[tokio::test]
async fn rate_limit_returns_429_with_retry_after() {
    let t = app().await;
    let secret = create_webhook(&t, "Spammer").await;
    // Cap is 60/min. Burst past it; the first Deny carries Retry-After.
    let mut hit_429 = false;
    for _ in 0..65 {
        let (status, retry) = post_webhook(&t.app, &secret, "{\"text\":\"x\"}").await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(retry.is_some(), "429 carries Retry-After");
            hit_429 = true;
            break;
        }
    }
    assert!(hit_429, "rate limit eventually triggers");
}

#[tokio::test]
async fn markdown_flag_controls_rendering() {
    let t = app().await;
    let secret = create_webhook(&t, "MD").await;
    // markdown:true -> stored raw, renders bold.
    post_webhook(&t.app, &secret, "{\"text\":\"**bold**\",\"markdown\":true}").await;
    let (_, body, _) = latest(&t.chat, t.room).await;
    assert_eq!(body, "**bold**");
    // markdown:false (default) -> escaped so it renders literally.
    post_webhook(&t.app, &secret, "{\"text\":\"**plain**\"}").await;
    let (_, body, _) = latest(&t.chat, t.room).await;
    assert!(
        body.contains("\\*"),
        "markdown metacharacters escaped: {body}"
    );
}

#[tokio::test]
async fn deleting_room_cascades_webhooks() {
    let t = app().await;
    let secret = create_webhook(&t, "Doomed").await;
    let hash = lets_chat::auth::hash_api_token(&SECRET, &secret);
    assert!(db::webhooks::find_by_secret_hash(&t.chat, &hash)
        .await
        .unwrap()
        .is_some());
    db::chat::delete_room(&t.chat, t.room).await.unwrap();
    assert!(
        db::webhooks::find_by_secret_hash(&t.chat, &hash)
            .await
            .unwrap()
            .is_none(),
        "webhook row removed when the room is deleted"
    );
}
