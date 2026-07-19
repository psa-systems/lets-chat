//! LC-516: removing the auto-default "General" enclave.
//!
//! Two behaviors are covered:
//! 1. A user who belongs to no enclave sees the inline "create your first
//!    enclave" prompt on Home; a user who belongs to one does not.
//! 2. An enclave manager can add a bot directly as a member (bots cannot
//!    accept invitations), and a non-manager cannot.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    auth: SqlitePool,
    chat: SqlitePool,
    alice_id: String,
    alice_session: String,
    bob_id: String,
    bob_session: String,
    carol_id: String,
    bot_id: String,
}

async fn setup() -> TestApp {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let carol_id = db::auth::create_user(&auth, "carol", "h").await.unwrap();
    // alice is a site admin so she is an enclave owner on the enclaves she
    // creates; bob is a plain user used for the non-manager path.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    let bot_id = db::auth::create_bot(&auth, "helperbot").await.unwrap();

    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();

    // Deliberately NOT calling backfill_general_membership: LC-516 removed the
    // auto-default enclave, so a fresh user starts with zero enclaves.

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
        secret_key: None,
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
    let app = routes::build_router(state);
    TestApp {
        app,
        auth,
        chat,
        alice_id,
        alice_session,
        bob_id,
        bob_session,
        carol_id,
        bot_id,
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
    let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn post_form(app: &Router, sess: &str, uri: &str, body: String) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn no_enclave_user_sees_create_prompt() {
    let t = setup().await;
    let (status, body) = get(&t.app, &t.bob_session, "/?home=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("action=\"/enclaves\""),
        "a user with no enclaves must see the inline create-enclave form",
    );
    assert!(
        body.contains("Create your first enclave"),
        "the create prompt title should render",
    );
}

#[tokio::test]
async fn enclave_member_does_not_see_prompt() {
    let t = setup().await;
    // Giving bob an enclave (as its owner) suppresses the prompt.
    db::enclave::create_enclave(&t.chat, "bobland", None, &t.bob_id)
        .await
        .unwrap();
    let (status, body) = get(&t.app, &t.bob_session, "/?home=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("Create your first enclave"),
        "a user who already belongs to an enclave must not see the create prompt",
    );
}

#[tokio::test]
async fn owner_can_add_bot_to_enclave() {
    let t = setup().await;
    let eid = db::enclave::create_enclave(&t.chat, "alicetown", None, &t.alice_id)
        .await
        .unwrap();

    let status = post_form(
        &t.app,
        &t.alice_session,
        &format!("/enclave/{eid}/members/add-bot"),
        format!("bot_id={}", t.bot_id),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let membership = db::enclave::get_membership(&t.chat, eid, &t.bot_id)
        .await
        .unwrap();
    assert!(
        membership.is_some(),
        "the bot should now be a member of the enclave",
    );
}

#[tokio::test]
async fn non_manager_cannot_add_bot() {
    let t = setup().await;
    // alice owns the enclave; bob is not a member, so cannot manage it.
    let eid = db::enclave::create_enclave(&t.chat, "alicetown", None, &t.alice_id)
        .await
        .unwrap();

    let status = post_form(
        &t.app,
        &t.bob_session,
        &format!("/enclave/{eid}/members/add-bot"),
        format!("bot_id={}", t.bot_id),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert!(
        db::enclave::get_membership(&t.chat, eid, &t.bot_id)
            .await
            .unwrap()
            .is_none(),
        "a non-manager must not be able to add a bot",
    );
}

#[tokio::test]
async fn add_bot_rejects_non_bot_user() {
    let t = setup().await;
    let eid = db::enclave::create_enclave(&t.chat, "alicetown", None, &t.alice_id)
        .await
        .unwrap();

    // carol is a normal user, not a bot; the add-bot path must reject her.
    let status = post_form(
        &t.app,
        &t.alice_session,
        &format!("/enclave/{eid}/members/add-bot"),
        format!("bot_id={}", t.carol_id),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    assert!(
        db::enclave::get_membership(&t.chat, eid, &t.carol_id)
            .await
            .unwrap()
            .is_none(),
        "a non-bot user must not be added via the add-bot route",
    );
}

#[tokio::test]
async fn add_bot_rejects_disabled_bot() {
    let t = setup().await;
    let eid = db::enclave::create_enclave(&t.chat, "alicetown", None, &t.alice_id)
        .await
        .unwrap();
    // Disabling a bot (via /admin/bots/{id}/disable) site-bans it; such a bot
    // must not be addable to an enclave.
    sqlx::query("UPDATE users SET is_banned=1 WHERE id=?")
        .bind(&t.bot_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let status = post_form(
        &t.app,
        &t.alice_session,
        &format!("/enclave/{eid}/members/add-bot"),
        format!("bot_id={}", t.bot_id),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        db::enclave::get_membership(&t.chat, eid, &t.bot_id)
            .await
            .unwrap()
            .is_none(),
        "a disabled bot must not be added",
    );
}

#[tokio::test]
async fn create_enclave_rejects_overlong_name() {
    let t = setup().await;
    let long = "x".repeat(81);
    let status = post_form(&t.app, &t.bob_session, "/enclaves", format!("name={long}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
