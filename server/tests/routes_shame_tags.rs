//! LC-342: HTTP coverage for shame tags - feature gating (off => 404), voting,
//! and the manager-gated moderator override.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-shame-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    member_session: String,
    admin_session: String,
    chat: SqlitePool,
    /// A message (authored by admin) in room 1 of the General enclave.
    message_id: i64,
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
    let message_id = db::chat::insert_message(&chat, 1, &admin_id, "hello")
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        member_session,
        admin_session,
        chat: chat_for_test,
        message_id,
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
async fn voting_404s_when_feature_off() {
    let t = app().await;
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{}/tags/spam", t.message_id),
        "",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "shame off => endpoint hidden"
    );
}

#[tokio::test]
async fn member_votes_when_enabled() {
    let t = app().await;
    db::enclave::set_shame_tags_enabled(&t.chat, 1, true)
        .await
        .unwrap();
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{}/tags/spam", t.message_id),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        db::shame_tags::tag_counts(&t.chat, t.message_id)
            .await
            .unwrap()
            .get("spam"),
        Some(&1)
    );

    // Unknown tag rejected.
    let bad = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{}/tags/bogus", t.message_id),
        "",
    )
    .await;
    assert_eq!(bad, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn override_is_manager_gated() {
    let t = app().await;
    db::enclave::set_shame_tags_enabled(&t.chat, 1, true)
        .await
        .unwrap();

    // Regular member cannot override.
    let blocked = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/messages/{}/tag-override", t.message_id),
        "state=hide",
    )
    .await;
    assert_eq!(blocked, StatusCode::FORBIDDEN);
    assert_eq!(
        db::shame_tags::get_override(&t.chat, t.message_id)
            .await
            .unwrap(),
        None
    );

    // Manager (admin/owner) can force-hide.
    let ok = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/messages/{}/tag-override", t.message_id),
        "state=hide",
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    assert_eq!(
        db::shame_tags::get_override(&t.chat, t.message_id)
            .await
            .unwrap(),
        Some(true)
    );
}
