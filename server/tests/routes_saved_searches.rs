//! LC-312: saved searches endpoints. Save a query, see it in the empty-query
//! popover, delete it. Auth-gated.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct TestApp {
    app: Router,
    session: String,
}

async fn setup() -> TestApp {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let uid = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        session,
    }
}

async fn post_form(
    app: &Router,
    sess: Option<&str>,
    uri: &str,
    body: String,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_empty_search(app: &Router, sess: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/search")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn save_then_appears_in_empty_popover_then_delete() {
    let t = setup().await;
    // Initially the empty-query popover is blank (no saved searches).
    assert_eq!(get_empty_search(&t.app, &t.session).await.trim(), "");

    let (status, _) = post_form(
        &t.app,
        Some(&t.session),
        "/searches",
        "query=from:bob+deploy".into(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let body = get_empty_search(&t.app, &t.session).await;
    assert!(
        body.contains("from:bob deploy"),
        "saved query not listed: {body}"
    );
    assert!(body.contains("data-lc-saved-query"), "no run hook: {body}");

    let (status, _) = post_form(
        &t.app,
        Some(&t.session),
        "/searches/delete",
        "query=from:bob+deploy".into(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        get_empty_search(&t.app, &t.session).await.trim(),
        "",
        "popover should be empty after delete"
    );
}

#[tokio::test]
async fn save_is_idempotent() {
    let t = setup().await;
    post_form(&t.app, Some(&t.session), "/searches", "query=cats".into()).await;
    post_form(&t.app, Some(&t.session), "/searches", "query=cats".into()).await;
    let body = get_empty_search(&t.app, &t.session).await;
    assert_eq!(
        body.matches("data-lc-saved-query").count(),
        1,
        "duplicate saved: {body}"
    );
}

#[tokio::test]
async fn unauthenticated_save_redirects() {
    let t = setup().await;
    let (status, _) = post_form(&t.app, None, "/searches", "query=x".into()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}
