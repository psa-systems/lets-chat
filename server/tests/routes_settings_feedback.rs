//! LC-426: settings forms return an htmx status fragment (inline status + an
//! out-of-band toast) when the request carries `HX-Request`, and keep their
//! full-page redirect otherwise (progressive enhancement). Exercised through
//! the language save; the same `hx_feedback` helper backs every save form.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn setup() -> (Router, String) {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
    (routes::build_router(state), session)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn hx_request_returns_status_fragment_and_toast() {
    let (app, session) = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/language")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .header("HX-Request", "true")
        .body(Body::from("locale="))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Inline status swapped into the form slot...
    assert!(body.contains("lc-status"), "missing inline status: {body}");
    // ...plus an out-of-band toast for the global region.
    assert!(
        body.contains("hx-swap-oob") && body.contains("lc-toast"),
        "missing oob toast: {body}"
    );
}

#[tokio::test]
async fn avatar_delete_hx_returns_single_oob_image_reset() {
    let (app, session) = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/avatar/delete")
        .header(header::COOKIE, format!("session={session}"))
        .header("HX-Request", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // LC-432: a single preview <img> is OOB-swapped back to the avatar URL -
    // no second placeholder element.
    assert!(
        body.contains("id=\"lc-set-avatar-img\"") && body.contains("hx-swap-oob"),
        "missing oob avatar reset: {body}"
    );
    assert!(
        !body.contains("lc-set-avatar-fallback"),
        "should not render a second avatar placeholder: {body}"
    );
}

#[tokio::test]
async fn non_hx_request_still_redirects() {
    let (app, session) = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/language")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("locale="))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    // Full-page submit keeps the redirect-with-flash behavior.
    assert!(
        resp.status().is_redirection(),
        "expected redirect, got {}",
        resp.status()
    );
}
