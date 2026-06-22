//! LC-85 integration: read-only / moderators-only room posting policy.
//!
//! Covers the server-side enforcement (a non-moderator's POST returns
//! 403 in a `moderators_only` room), the happy path (after a moderator
//! override grants the user mod power, the same POST succeeds), and an
//! audit-log spot-check (the policy flip records a `room_posting_policy`
//! row in `mod_actions`).
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
            let p = std::env::temp_dir().join(format!("lc-readonly-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    member_id: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
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
        bg: bg.clone(),
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        member_id,
        chat: chat_for_test,
    }
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn admin_can_set_posting_policy() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/posting-policy",
        "policy=moderators_only",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let stored: String = sqlx::query_scalar("SELECT posting_allowed_for FROM rooms WHERE id=1")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(stored, "moderators_only");
    let _ = t.admin_id;
    let _ = t.member_id;
}

#[tokio::test]
async fn plain_member_cannot_change_posting_policy() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/posting-policy",
        "policy=moderators_only",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_policy_rejected() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/posting-policy",
        "policy=nonsense",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn member_post_blocked_in_moderators_only_room() {
    let t = app().await;
    // Admin flips to moderators-only.
    assert_eq!(
        send(
            &t.app,
            &t.admin_session,
            Method::POST,
            "/room/1/posting-policy",
            "policy=moderators_only"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    // Member tries to post a normal message - 403.
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        "body=hello&file_id=",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Admin can still post.
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/messages",
        "body=announcement&file_id=",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn moderator_override_unlocks_moderators_only_room() {
    let t = app().await;
    // Flip policy.
    send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/posting-policy",
        "policy=moderators_only",
    )
    .await;
    // Grant member a moderator override (LC-84).
    let body = format!("user_id={}&role=moderator", t.member_id);
    send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/moderators",
        &body,
    )
    .await;
    // Member now CAN post.
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        "body=hello&file_id=",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn policy_change_writes_audit_log() {
    let t = app().await;
    send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/posting-policy",
        "policy=moderators_only",
    )
    .await;
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let row = actions
        .iter()
        .find(|a| a.action == "room_posting_policy")
        .expect("policy flip audit row");
    assert_eq!(row.room_id, Some(1));
    assert_eq!(row.actor_user, t.admin_id);
    assert!(row
        .metadata
        .as_deref()
        .unwrap_or("")
        .contains("moderators_only"));
}
