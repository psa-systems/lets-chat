//! LC-537: image alt text. Covers the storage round-trip (`db::uploads`) and
//! the author-gated POST /api/files/{id}/alt editor. Harness mirrors
//! routes_stats / routes_hovercard.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

#[tokio::test]
async fn alt_text_round_trips_and_falls_back_to_filename() {
    let chat = common::chat_pool().await;
    let msg = db::chat::insert_message(&chat, 1, "u1", "look")
        .await
        .unwrap();
    let up =
        db::uploads::insert_upload(&chat, "u1", "cat.png", "image/png", 10, "/p/cat.png", None)
            .await
            .unwrap();
    db::uploads::link_upload_to_message(&chat, up, msg)
        .await
        .unwrap();

    // Unset: alt() falls back to the filename.
    let a = &db::uploads::attachments_for_message(&chat, msg)
        .await
        .unwrap()[0];
    assert_eq!(a.alt(), "cat.png");
    assert!(!a.has_alt());

    // Set, then read back.
    db::uploads::set_upload_alt_text(&chat, up, Some("a napping cat"))
        .await
        .unwrap();
    let a = &db::uploads::attachments_for_message(&chat, msg)
        .await
        .unwrap()[0];
    assert_eq!(a.alt(), "a napping cat");
    assert!(a.has_alt());

    // Clear back to None: fallback returns.
    db::uploads::set_upload_alt_text(&chat, up, None)
        .await
        .unwrap();
    let a = &db::uploads::attachments_for_message(&chat, msg)
        .await
        .unwrap()[0];
    assert_eq!(a.alt(), "cat.png");
}

struct Setup {
    app: Router,
    author_session: String,
    other_session: String,
    file_id: i64,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let author = db::auth::create_user(&auth, "author", "h").await.unwrap();
    let other = db::auth::create_user(&auth, "other", "h").await.unwrap();
    // An admin must exist so backfill grants General membership (room access).
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&author)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let msg = db::chat::insert_message(&chat, 1, &author, "look")
        .await
        .unwrap();
    let file_id = db::uploads::insert_upload(
        &chat,
        &author,
        "cat.png",
        "image/png",
        10,
        "/p/cat.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&chat, file_id, msg)
        .await
        .unwrap();

    let author_session = db::auth::create_session(&auth, &author).await.unwrap();
    let other_session = db::auth::create_session(&auth, &other).await.unwrap();

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
    Setup {
        app: routes::build_router(state),
        author_session,
        other_session,
        file_id,
    }
}

async fn post_alt(
    app: &Router,
    sess: Option<&str>,
    file_id: i64,
    alt: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/files/{file_id}/alt"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let body = format!("alt={}", urlencoding_min(alt));
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

// Minimal form-encode: spaces to '+' is enough for these test strings.
fn urlencoding_min(s: &str) -> String {
    s.replace(' ', "+")
}

#[tokio::test]
async fn author_sets_alt_and_gets_rerendered_message() {
    let s = setup().await;
    let (status, body) =
        post_alt(&s.app, Some(&s.author_session), s.file_id, "a napping cat").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("a napping cat"),
        "re-rendered alt missing: {body}"
    );
}

#[tokio::test]
async fn non_author_cannot_set_alt() {
    let s = setup().await;
    let (status, _) = post_alt(&s.app, Some(&s.other_session), s.file_id, "hijack").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unauthenticated_is_redirected() {
    let s = setup().await;
    let (status, _) = post_alt(&s.app, None, s.file_id, "anon").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}
