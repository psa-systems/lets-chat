//! LC-739: the enclave settings forms post via htmx, so every handler behind
//! them has to answer two callers. These tests pin both halves of that contract:
//! an `HX-Request` gets a fragment (the shared inline status + toast) or an
//! `HX-Redirect` where the save changes content a status fragment cannot patch,
//! and a submit WITHOUT the header still gets the plain 303 the no-JS path has
//! always had.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// A site admin (so `require_manage` passes on the seeded General enclave, id 1)
/// with the whole router wired to in-memory pools.
async fn app() -> (Router, String, SqlitePool) {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let user_id = db::auth::create_user(&auth, "owner", "hash").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
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
    (routes::build_router(state), session, chat)
}

/// POST a form body; `hx` decides whether the request carries `HX-Request`.
async fn post(app: &Router, sess: &str, uri: &str, body: &'static str, hx: bool) -> Response {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("cookie", format!("session={sess}"))
        .header("content-type", "application/x-www-form-urlencoded");
    if hx {
        req = req.header("hx-request", "true");
    }
    app.clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn body_string(res: Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// A save whose new state the form controls already carry answers htmx with the
/// shared feedback fragment: the inline status AND the out-of-band toast.
#[tokio::test]
async fn toggle_save_answers_htmx_with_inline_status_and_toast() {
    let (app, sess, chat) = app().await;
    let res = post(&app, &sess, "/enclave/1/coyote-mode", "enabled=1", true).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("lc-status--ok"), "inline status: {body}");
    assert!(
        body.contains(r#"hx-swap-oob="beforeend:#lc-toast-region""#),
        "toast: {body}"
    );
    let e = db::enclave::get_enclave(&chat, 1).await.unwrap().unwrap();
    assert!(e.coyote_mode);
}

/// The same POST without the header is the no-JS path, unchanged: 303 back to
/// the settings page with the `?ok=` flash code.
#[tokio::test]
async fn toggle_save_without_htmx_still_redirects() {
    let (app, sess, chat) = app().await;
    let res = post(&app, &sess, "/enclave/1/coyote-mode", "enabled=1", false).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap(),
        "/enclave/1/settings?ok=updated"
    );
    let e = db::enclave::get_enclave(&chat, 1).await.unwrap().unwrap();
    assert!(e.coyote_mode);
}

/// The toggles are checkboxes now, and an unchecked box posts no field at all.
/// That has to read as "off", not as a malformed submit.
#[tokio::test]
async fn unchecked_toggle_checkbox_turns_the_setting_off() {
    let (app, sess, chat) = app().await;
    post(&app, &sess, "/enclave/1/coyote-mode", "enabled=1", true).await;
    post(&app, &sess, "/enclave/1/share-emojis", "share=1", true).await;

    let res = post(&app, &sess, "/enclave/1/coyote-mode", "", true).await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = post(&app, &sess, "/enclave/1/share-emojis", "", true).await;
    assert_eq!(res.status(), StatusCode::OK);

    let e = db::enclave::get_enclave(&chat, 1).await.unwrap().unwrap();
    assert!(!e.coyote_mode, "absent field must disable coyote mode");
    assert!(
        !e.share_emojis_globally,
        "absent field must disable emoji sharing"
    );
}

/// Rotating the invite code replaces content the status fragment cannot patch,
/// so htmx is told to navigate to the same URL the no-JS submit redirects to.
#[tokio::test]
async fn content_changing_save_answers_htmx_with_hx_redirect() {
    let (app, sess, _chat) = app().await;
    let res = post(&app, &sess, "/enclave/1/invite-code", "", true).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("hx-redirect").unwrap(),
        "/enclave/1/settings?ok=rotated"
    );

    let res = post(&app, &sess, "/enclave/1/invite-code", "", false).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap(),
        "/enclave/1/settings?ok=rotated"
    );
}

/// A rejected save reaches an htmx submit as an inline error plus a toast, with
/// the reason intact, instead of the flash banner only a reload would show.
#[tokio::test]
async fn failed_save_answers_htmx_with_inline_error_and_toast() {
    let (app, sess, _chat) = app().await;
    // Claim the name on a second enclave so renaming General to it collides.
    let res = post(&app, &sess, "/enclaves", "name=taken", false).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let res = post(&app, &sess, "/enclave/1/edit", "name=taken", true).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("lc-status--err"), "inline error: {body}");
    assert!(
        body.contains(r#"hx-swap-oob="beforeend:#lc-toast-region""#),
        "toast: {body}"
    );
    assert!(body.contains("already exists"), "reason kept: {body}");
}
