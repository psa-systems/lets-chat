#![cfg(feature = "standalone")]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn make_app() -> (Router, String, SqlitePool) {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let user_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
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
    (routes::build_router(state), session, auth)
}

#[tokio::test]
async fn logout_succeeds_and_clears_cookie_when_delete_session_works() {
    let (app, session, auth) = make_app().await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/logout")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let session_user = db::auth::get_user_by_session(&auth, &session)
        .await
        .unwrap();
    assert!(
        session_user.is_none(),
        "session row should be gone after a successful logout"
    );
}

#[tokio::test]
async fn logout_returns_error_and_does_not_lose_delete_session_failure() {
    let (app, session, auth) = make_app().await;

    // Close the auth pool so `delete_session` fails with a real DB error
    // instead of silently discarding it, as `let _ =` used to.
    auth.close().await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/logout")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // The cookie is still cleared for the browser even though the server-side
    // session row could not be deleted.
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(set_cookie.contains("session="));
}
