//! LC-1016: `require_post_access` (used by the poll/event compose routes)
//! must enforce the same send gates as the text-message post path. Before
//! this fix it only checked banned/muted + room access, so an enclave-banned
//! member (LC-339) or a member subject to slowmode (LC-534) could still post
//! a poll where a text message from the same user in the same room would be
//! rejected.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::push::MockPushClient;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes, state::AppState};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-poll-gates-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    user_id: String,
    session: String,
    chat: SqlitePool,
    enclave_id: i64,
    room: i64,
}

async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let owner = db::auth::create_user(&auth, "owner", "h").await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &owner)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "announcements", None, "public", None, Some(eid))
        .await
        .unwrap();

    let user_id = db::auth::create_user(&auth, "mallory", "h").await.unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::add_member(&chat, eid, &user_id, EnclaveRole::Member)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
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
        push_client: Arc::new(MockPushClient::default()),
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
        user_id,
        session,
        chat,
        enclave_id: eid,
        room,
    }
}

async fn post_poll(app: &Router, room_id: i64, sess: &str, question: &str) -> StatusCode {
    let form = format!("question={question}&options=A%0AB");
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/poll"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn message_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn enclave_banned_member_cannot_post_a_poll() {
    let t = setup().await;
    db::enclave::ban_from_enclave(&t.chat, t.enclave_id, &t.user_id, "spam")
        .await
        .unwrap();

    let status = post_poll(&t.app, t.room, &t.session, "shall+we").await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(message_count(&t.chat).await, 0);
}

#[tokio::test]
async fn slowmode_throttles_a_second_poll_post() {
    let t = setup().await;
    db::chat::set_room_slowmode(&t.chat, t.room, 5)
        .await
        .unwrap();

    assert!(post_poll(&t.app, t.room, &t.session, "first")
        .await
        .is_success());
    // Immediate second poll is inside the 5s slowmode cooldown.
    assert_eq!(
        post_poll(&t.app, t.room, &t.session, "second").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(message_count(&t.chat).await, 1);
}
