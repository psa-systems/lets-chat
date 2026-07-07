//! LC-547: ephemeral / self-destruct messages. The unconditional sweep hard-
//! deletes only messages whose expires_at is in the past, and post_message
//! stamps expires_at from the composer's TTL token (and only from the closed
//! allowlist).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::retention::sweep::sweep_expired_ephemeral;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

async fn expiry_of(chat: &sqlx::SqlitePool, id: i64) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT expires_at FROM messages WHERE id = ?")
        .bind(id)
        .fetch_one(chat)
        .await
        .unwrap()
}

async fn row_exists(chat: &sqlx::SqlitePool, id: i64) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(id)
        .fetch_one(chat)
        .await
        .unwrap()
        > 0
}

#[tokio::test]
async fn sweep_deletes_only_past_expiry() {
    let chat = common::chat_pool().await;
    let past = db::chat::insert_message(&chat, 1, "u1", "gone soon")
        .await
        .unwrap();
    let future = db::chat::insert_message(&chat, 1, "u1", "still here")
        .await
        .unwrap();
    let permanent = db::chat::insert_message(&chat, 1, "u1", "forever")
        .await
        .unwrap();
    db::chat::set_message_expiry(&chat, past, "2000-01-01 00:00:00")
        .await
        .unwrap();
    db::chat::set_message_expiry(&chat, future, "2999-01-01 00:00:00")
        .await
        .unwrap();

    let stats = sweep_expired_ephemeral(&chat).await.unwrap();
    assert_eq!(
        stats.messages_deleted, 1,
        "only the past-expiry row deleted"
    );
    assert_eq!(stats.purged, vec![(past, 1)]);

    assert!(
        !row_exists(&chat, past).await,
        "expired row is hard-deleted"
    );
    assert!(
        row_exists(&chat, future).await,
        "future-expiry row survives"
    );
    assert!(
        row_exists(&chat, permanent).await,
        "permanent (NULL expiry) row survives"
    );
}

#[tokio::test]
async fn sweep_is_a_noop_when_nothing_has_expired() {
    let chat = common::chat_pool().await;
    let m = db::chat::insert_message(&chat, 1, "u1", "hi")
        .await
        .unwrap();
    db::chat::set_message_expiry(&chat, m, "2999-01-01 00:00:00")
        .await
        .unwrap();

    let stats = sweep_expired_ephemeral(&chat).await.unwrap();
    assert_eq!(stats.messages_deleted, 0);
    assert!(stats.purged.is_empty());
    assert!(row_exists(&chat, m).await);
}

// ---- Route-level: post_message stamps expiry from the TTL token. ----

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-ephemeral-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

struct TestApp {
    app: Router,
    session: String,
    chat: sqlx::SqlitePool,
}

async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let uid = db::auth::create_user(&auth, "poster", "h").await.unwrap();
    // Admin so backfill_general_membership (which no-ops without an admin)
    // seeds room membership; posting_allowed_for defaults to 'all'.
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
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
        llm_client: None,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        session,
        chat,
    }
}

async fn post(app: &Router, sess: &str, form: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn latest_message_id(chat: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM messages ORDER BY id DESC LIMIT 1")
        .fetch_one(chat)
        .await
        .unwrap()
}

#[tokio::test]
async fn post_with_ttl_stamps_future_expiry() {
    let t = setup().await;
    assert!(post(&t.app, &t.session, "body=secret&ttl=1h")
        .await
        .is_success());
    let id = latest_message_id(&t.chat).await;
    let exp = expiry_of(&t.chat, id).await.expect("expiry stamped");
    // Stamp is in the "%Y-%m-%d %H:%M:%S" UTC shape and strictly in the future.
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    assert!(exp > now, "expiry {exp} should be after now {now}");
}

#[tokio::test]
async fn post_without_ttl_is_permanent() {
    let t = setup().await;
    assert!(post(&t.app, &t.session, "body=keep&ttl=")
        .await
        .is_success());
    let id = latest_message_id(&t.chat).await;
    assert_eq!(expiry_of(&t.chat, id).await, None, "empty ttl = permanent");
}

#[tokio::test]
async fn post_with_forged_ttl_is_permanent() {
    let t = setup().await;
    assert!(post(&t.app, &t.session, "body=keep&ttl=100000d")
        .await
        .is_success());
    let id = latest_message_id(&t.chat).await;
    assert_eq!(
        expiry_of(&t.chat, id).await,
        None,
        "a token outside the allowlist must not stamp an expiry"
    );
}
