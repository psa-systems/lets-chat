//! LC-504: assert the six security response headers are present on every
//! response shape - a public 200, an authed page 200 and an unauthenticated
//! redirect - since `set_security_headers` is the outermost router layer.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// The exact header names LC-504 requires on every response.
const REQUIRED_HEADERS: &[&str] = &[
    "strict-transport-security",
    "x-content-type-options",
    "x-frame-options",
    "referrer-policy",
    "content-security-policy",
    "permissions-policy",
];

async fn app() -> (Router, String) {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    // Promote so `backfill_general_membership` runs (it early-returns with no
    // admin) and the user actually has room access on the home page.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
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
    (routes::build_router(state), session)
}

async fn get(app: &Router, uri: &str, session: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(s) = session {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn assert_all_headers(resp: &axum::response::Response, ctx: &str) {
    for name in REQUIRED_HEADERS {
        assert!(
            resp.headers().contains_key(*name),
            "missing `{name}` on {ctx} (status {})",
            resp.status()
        );
    }
}

#[tokio::test]
async fn headers_present_on_public_200() {
    let (app, _session) = app().await;
    let resp = get(&app, "/version", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_all_headers(&resp, "GET /version");
}

#[tokio::test]
async fn headers_present_on_authed_home() {
    let (app, session) = app().await;
    let resp = get(&app, "/", Some(&session)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_all_headers(&resp, "GET / (authed)");
}

#[tokio::test]
async fn headers_present_on_redirect_response() {
    // An unauthenticated request to a gated path redirects (303) rather than
    // returning a handler body; the outermost layer must still set the headers
    // on the redirect.
    let (app, _session) = app().await;
    let resp = get(&app, "/no-such-path-lc504", None).await;
    assert!(
        resp.status().is_redirection(),
        "expected a redirect, got {}",
        resp.status()
    );
    assert_all_headers(&resp, "GET /no-such-path-lc504 (redirect)");
}

#[tokio::test]
async fn csp_and_framing_values_are_locked_down() {
    let (app, _session) = app().await;
    let resp = get(&app, "/version", None).await;

    let xfo = resp.headers().get("x-frame-options").unwrap();
    assert_eq!(xfo, "DENY");

    let nosniff = resp.headers().get("x-content-type-options").unwrap();
    assert_eq!(nosniff, "nosniff");

    let csp = resp
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    // The hardening wins: no framing, no plugins, locked base + form targets.
    assert!(csp.contains("frame-ancestors 'none'"), "csp: {csp}");
    assert!(csp.contains("object-src 'none'"), "csp: {csp}");
    assert!(csp.contains("base-uri 'self'"), "csp: {csp}");
    // LC-507: the GIF picker grid hotlinks Giphy CDN thumbnails, so img-src must
    // allow the Giphy CDN (and must not still point at the dead Tenor CDN).
    assert!(csp.contains("https://*.giphy.com"), "csp: {csp}");
    assert!(!csp.contains("tenor.com"), "csp: {csp}");

    let perms = resp
        .headers()
        .get("permissions-policy")
        .unwrap()
        .to_str()
        .unwrap();
    // WebRTC surface keeps camera/mic; geolocation is denied.
    assert!(perms.contains("camera=(self)"), "perms: {perms}");
    assert!(perms.contains("geolocation=()"), "perms: {perms}");
}
