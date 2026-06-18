//! LC-217: per-enclave message send rate-limit override.
//!
//! Three cases:
//!  - Zero override falls through to the global cap (no per-enclave deny).
//!  - Non-zero override blocks at its threshold when global is higher.
//!  - The limit is per-enclave: a user blocked in enclave A can still post
//!    in enclave B with a different threshold (counter keys are scoped by
//!    enclave id).
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-encratelimit-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    session: String,
    user_id: String,
    chat: SqlitePool,
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
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        session,
        user_id,
        chat: chat_for_test,
    }
}

async fn create_enclave_with_room(chat: &SqlitePool, name: &str, owner: &str) -> (i64, i64) {
    let enclave_id = db::enclave::create_enclave(chat, name, None, owner)
        .await
        .unwrap();
    let room_id = db::chat::create_room(chat, "general", None, "public", None, Some(enclave_id))
        .await
        .unwrap();
    (enclave_id, room_id)
}

async fn post_msg(app: &Router, sess: &str, room_id: i64, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(format!("body={body}")))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn zero_override_falls_through_to_global() {
    let t = app().await;
    let (enclave_id, room_id) = create_enclave_with_room(&t.chat, "alpha", &t.user_id).await;
    db::enclave::set_msg_rate_limit_burst(&t.chat, enclave_id, 0)
        .await
        .unwrap();
    // Global is 0 (unset) -> no limit applies. 10 fast posts all succeed.
    for i in 0..10 {
        let s = post_msg(&t.app, &t.session, room_id, &format!("hi-{i}")).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "post {i} expected 204, got {s}");
    }
}

#[tokio::test]
async fn nonzero_override_blocks_at_threshold() {
    let t = app().await;
    let (enclave_id, room_id) = create_enclave_with_room(&t.chat, "beta", &t.user_id).await;
    db::enclave::set_msg_rate_limit_burst(&t.chat, enclave_id, 3)
        .await
        .unwrap();
    // First 3 succeed.
    for i in 0..3 {
        let s = post_msg(&t.app, &t.session, room_id, &format!("ok-{i}")).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "post {i} expected 204, got {s}");
    }
    // Fourth hits the 429 from the per-enclave cap.
    let s = post_msg(&t.app, &t.session, room_id, "blocked").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "4th post should 429");
}

#[tokio::test]
async fn limits_are_per_enclave() {
    let t = app().await;
    let (eid_a, room_a) = create_enclave_with_room(&t.chat, "gamma", &t.user_id).await;
    let (eid_b, room_b) = create_enclave_with_room(&t.chat, "delta", &t.user_id).await;
    db::enclave::set_msg_rate_limit_burst(&t.chat, eid_a, 2)
        .await
        .unwrap();
    db::enclave::set_msg_rate_limit_burst(&t.chat, eid_b, 5)
        .await
        .unwrap();
    // Burn enclave A to its cap.
    assert_eq!(
        post_msg(&t.app, &t.session, room_a, "a1").await,
        StatusCode::NO_CONTENT,
        "A1 expected 204"
    );
    assert_eq!(
        post_msg(&t.app, &t.session, room_a, "a2").await,
        StatusCode::NO_CONTENT,
        "A2 expected 204"
    );
    assert_eq!(
        post_msg(&t.app, &t.session, room_a, "a3").await,
        StatusCode::TOO_MANY_REQUESTS,
        "A 3rd should 429"
    );
    // Enclave B has its own counter; 5 more should succeed.
    for i in 0..5 {
        let s = post_msg(&t.app, &t.session, room_b, &format!("b-{i}")).await;
        assert_eq!(
            s,
            StatusCode::NO_CONTENT,
            "B post {i} expected 204, got {s}"
        );
    }
    // 6th in B hits its cap.
    let s = post_msg(&t.app, &t.session, room_b, "b-6").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "B 6th should 429");
}
