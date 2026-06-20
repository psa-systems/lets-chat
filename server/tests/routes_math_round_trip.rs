//! LC-59 route-level round-trip coverage.
//!
//! The acceptance criterion is that editing or quoting a message preserves
//! the SOURCE math (`$x^2$`), not the rendered MathML. Math is a
//! render-time transform; the `body` column stores raw markdown and every
//! consumer reads from that column. These tests pin the contract end-to-
//! end: POST a math message, then assert the edit form and the
//! composer-quote chip both surface the raw `$...$`.
//!
//! The unit-level "render is pure on its input" pin was a tautology (the
//! Rust type system already guarantees `&str` is immutable). The real
//! round-trip property lives at the route layer, which is what this file
//! covers.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir()
                .join(format!("lc-math-round-trip-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

struct TestApp {
    app: Router,
    session: String,
    chat: SqlitePool,
}

async fn app_with_user(username: &str) -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let user_id = db::auth::create_user(&auth, username, "hash")
        .await
        .unwrap();
    // Promote to admin so backfill_general_membership runs (it early-returns
    // when no admin exists). Set totp_enabled=1 so the enforce_2fa_enrollment
    // middleware does not redirect every authed request to /settings/2fa/setup
    // (AppState below sets secret_key which activates that middleware).
    // Matches the workaround in routes_message_edit_history.rs.
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
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
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
        session,
        chat: chat_for_test,
    }
}

/// Form-encode a string with full support for the characters that appear
/// in LaTeX math (`$`, `^`, `\`, `{`, `}`, etc.). The helper in
/// `routes_message_edit_history.rs` only handles a small alphabet, so
/// LC-59 tests bring their own encoder.
fn form_encode(body: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    for b in body.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn post_message(app: &Router, sess: &str, room_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}&file_id=", form_encode(body));
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn get_edit_form(app: &Router, sess: &str, message_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/messages/{message_id}/edit"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get_quote_chip(
    app: &Router,
    sess: &str,
    room_id: i64,
    message_id: i64,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/room/{room_id}/composer-quote/{message_id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn last_message_id(chat: &SqlitePool, room_id: i64) -> i64 {
    let row = sqlx::query("SELECT id FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1")
        .bind(room_id)
        .fetch_one(chat)
        .await
        .unwrap();
    row.get::<i64, _>("id")
}

#[tokio::test]
async fn edit_form_contains_raw_inline_math_source() {
    // Post a message with inline math, fetch the edit form, assert the
    // raw `$x^2$` appears in the response (not MathML, not typeset).
    let t = app_with_user("mathuser").await;
    let body = "energy: $E = mc^2$";
    assert_eq!(
        post_message(&t.app, &t.session, 1, body).await,
        StatusCode::NO_CONTENT,
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, html) = get_edit_form(&t.app, &t.session, mid).await;
    assert_eq!(status, StatusCode::OK);
    // The raw source appears verbatim. None of `$`, `^`, `=`, ` ` are
    // HTML-escapable (askama escapes only `<`, `>`, `&`, `"`, `'`), so the
    // textarea contains the source string literally.
    assert!(
        html.contains("$E = mc^2$"),
        "edit form lost raw math source: {html}",
    );
    // Defense in depth: no MathML element leaked into the edit form -
    // the edit textarea must show source, not rendered output.
    assert!(
        !html.contains("<math"),
        "edit form contains rendered math instead of source: {html}",
    );
}

#[tokio::test]
async fn edit_form_contains_raw_display_math_source() {
    let t = app_with_user("displaymath").await;
    let body = "see: $$\\int_0^1 f(x)\\,dx$$";
    assert_eq!(
        post_message(&t.app, &t.session, 1, body).await,
        StatusCode::NO_CONTENT,
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, html) = get_edit_form(&t.app, &t.session, mid).await;
    assert_eq!(status, StatusCode::OK);
    // Raw display delimiters and the backslashes survive intact.
    assert!(
        html.contains(r"$$\int_0^1 f(x)\,dx$$"),
        "edit form lost raw display math source: {html}",
    );
    assert!(
        !html.contains("<math"),
        "edit form contains rendered math: {html}",
    );
}

#[tokio::test]
async fn composer_quote_chip_contains_raw_math_source() {
    // Post a math message, then GET the composer-quote chip for it.
    // The chip's body_excerpt comes from `excerpt_for_quote` which only
    // collapses newlines and truncates - no markdown rendering. The
    // raw `$x^2$` must survive.
    let t = app_with_user("quoter").await;
    let body = "key formula: $x^2 + y^2 = z^2$";
    assert_eq!(
        post_message(&t.app, &t.session, 1, body).await,
        StatusCode::NO_CONTENT,
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status, html) = get_quote_chip(&t.app, &t.session, 1, mid).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("$x^2 + y^2 = z^2$"),
        "quote chip lost raw math source: {html}",
    );
    assert!(
        !html.contains("<math"),
        "quote chip contains rendered math: {html}",
    );
}

#[tokio::test]
async fn edit_form_round_trip_then_resubmit_preserves_math() {
    // Post a math message; fetch the edit form; confirm the source is in
    // the textarea. Round-trip through the edit endpoint by PATCHing the
    // same body back and refetching - the message body still reads as
    // raw `$...$`, never baked-in MathML.
    let t = app_with_user("rt").await;
    let body = "$x^2$";
    assert_eq!(
        post_message(&t.app, &t.session, 1, body).await,
        StatusCode::NO_CONTENT,
    );
    let mid = last_message_id(&t.chat, 1).await;

    let (status1, html1) = get_edit_form(&t.app, &t.session, mid).await;
    assert_eq!(status1, StatusCode::OK);
    assert!(
        html1.contains("$x^2$"),
        "first edit form missing source: {html1}",
    );

    // Re-submit the same body. (PATCH /messages/{id} accepts form-encoded
    // body; matches the helper in routes_message_edit_history.rs.)
    let patch_form = format!("body={}", form_encode(body));
    let resp = t
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/messages/{mid}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("session={}", t.session))
                .body(Body::from(patch_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_success() || resp.status() == StatusCode::SEE_OTHER,
        "patch status: {}",
        resp.status(),
    );

    let (status2, html2) = get_edit_form(&t.app, &t.session, mid).await;
    assert_eq!(status2, StatusCode::OK);
    assert!(
        html2.contains("$x^2$"),
        "round-trip lost the raw source: {html2}",
    );
}
