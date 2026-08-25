//! LC-766 integration: the first-entry handle gate.
//!
//! A newly provisioned (SSO) user whose handle is still the derived value is
//! redirected to /welcome/handle on every authenticated page until they confirm
//! a handle; confirming releases the gate. A user who already has a confirmed
//! handle is never gated.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    /// Session for a user provisioned via SSO (handle unconfirmed -> gated).
    fresh_session: String,
    fresh_handle: String,
    /// Session for an established user (handle confirmed -> never gated).
    confirmed_session: String,
}

async fn app() -> TestApp {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    // SSO provisioning leaves username_confirmed_at NULL -> the gate should fire.
    let fresh = db::auth::create_user_from_bunyip(&auth, "fresh.user", "sub-fresh", None, None)
        .await
        .unwrap();
    let fresh_session = db::auth::create_session(&auth, &fresh).await.unwrap();

    // The legacy/test path stamps the handle confirmed -> no gate.
    let confirmed = db::auth::create_user(&auth, "confirmed", "h")
        .await
        .unwrap();
    let confirmed_session = db::auth::create_session(&auth, &confirmed).await.unwrap();

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
        fresh_session,
        fresh_handle: "fresh.user".to_string(),
        confirmed_session,
    }
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    (status, loc)
}

async fn post_form(
    app: &Router,
    sess: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    (status, loc)
}

#[tokio::test]
async fn unconfirmed_user_is_redirected_to_the_handle_prompt() {
    let t = app().await;
    let (status, loc) = get(&t.app, &t.fresh_session, "/settings").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(loc.as_deref(), Some("/welcome/handle"));
}

#[tokio::test]
async fn the_prompt_page_itself_is_reachable_while_unconfirmed() {
    let t = app().await;
    let (status, _) = get(&t.app, &t.fresh_session, "/welcome/handle").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn confirmed_user_is_not_gated() {
    let t = app().await;
    let (status, loc) = get(&t.app, &t.confirmed_session, "/settings").await;
    // Either a normal render, or some other redirect - but never to the prompt.
    assert_ne!(loc.as_deref(), Some("/welcome/handle"));
    assert_ne!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn confirming_the_handle_releases_the_gate() {
    let t = app().await;
    // Accept the derived handle.
    let (status, loc) = post_form(
        &t.app,
        &t.fresh_session,
        "/welcome/handle",
        &format!("username={}", t.fresh_handle),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(loc.as_deref(), Some("/"));

    // The gate no longer fires on an authenticated page.
    let (_, loc2) = get(&t.app, &t.fresh_session, "/settings").await;
    assert_ne!(loc2.as_deref(), Some("/welcome/handle"));
}
