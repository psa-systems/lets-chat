//! LC-102: per-room RSS/Atom + iCal feeds. Admin mints a feed (URL revealed
//! once); GET serves a valid document; revoke -> 410; a creator who can't read
//! the room -> 410; wrong-kind / unknown token -> 404; conditional GET -> 304.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [9u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-feed-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    room: i64,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
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
    };
    TestApp {
        app: routes::build_router(state),
        admin_session,
        room,
        chat,
        auth,
    }
}

/// Mint a feed of `kind` and return the token from the once-shown URL.
async fn mint(t: &TestApp, kind: &str) -> String {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{}/feeds", t.room))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(format!("kind={kind}")))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    let needle = format!("/feed/{kind}/");
    let pos = html.find(&needle).expect("feed url shown");
    let tail = &html[pos + needle.len()..];
    let end = tail
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(tail.len());
    tail[..end].to_string()
}

async fn get_feed(
    app: &Router,
    kind: &str,
    token: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, Option<String>, String) {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(format!("/feed/{kind}/{token}"));
    if let Some(inm) = if_none_match {
        b = b.header(header::IF_NONE_MATCH, inm);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let etag = res
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, etag, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn rss_feed_serves_atom_with_message() {
    let t = app().await;
    db::chat::insert_message(&t.chat, t.room, "u1", "hello feed world")
        .await
        .unwrap();
    let token = mint(&t, "rss").await;
    let (status, etag, body) = get_feed(&t.app, "rss", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(etag.is_some(), "ETag set");
    assert!(body.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\">"));
    assert!(body.contains("hello feed world"), "message in feed: {body}");
    assert!(
        body.contains(&format!("/room/{}#msg-", t.room)),
        "permalink"
    );
}

#[tokio::test]
async fn ical_feed_serves_calendar() {
    let t = app().await;
    let token = mint(&t, "ical").await;
    let (status, _, body) = get_feed(&t.app, "ical", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(body.contains("END:VCALENDAR"));
}

#[tokio::test]
async fn revoked_feed_is_410() {
    let t = app().await;
    let token = mint(&t, "rss").await;
    assert_eq!(
        get_feed(&t.app, "rss", &token, None).await.0,
        StatusCode::OK
    );
    let fid: i64 = sqlx::query_scalar("SELECT id FROM room_feeds WHERE room_id=?")
        .bind(t.room)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{}/feeds/{}/revoke", t.room, fid))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        t.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        get_feed(&t.app, "rss", &token, None).await.0,
        StatusCode::GONE
    );
}

#[tokio::test]
async fn conditional_get_returns_304() {
    let t = app().await;
    db::chat::insert_message(&t.chat, t.room, "u1", "x")
        .await
        .unwrap();
    let token = mint(&t, "rss").await;
    let (_, etag, _) = get_feed(&t.app, "rss", &token, None).await;
    let etag = etag.expect("etag");
    let (status, _, _) = get_feed(&t.app, "rss", &token, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn wrong_kind_and_unknown_token_are_404() {
    let t = app().await;
    let token = mint(&t, "rss").await;
    // Token minted for rss must not serve the ical path.
    assert_eq!(
        get_feed(&t.app, "ical", &token, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_feed(&t.app, "rss", "lc_nope", None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn creator_without_room_access_is_410() {
    let t = app().await;
    // A real user who is NOT a member of the room's enclave.
    let bob = db::auth::create_user(&t.auth, "bob", "h").await.unwrap();
    let token = "tok_bob_outsider";
    let hash = lets_chat::auth::hash_api_token(&SECRET, token);
    db::room_feeds::insert(&t.chat, t.room, "rss", &hash, &bob)
        .await
        .unwrap();
    assert_eq!(
        get_feed(&t.app, "rss", token, None).await.0,
        StatusCode::GONE,
        "feed bound to a non-member returns 410"
    );
}

#[tokio::test]
async fn deleted_creator_is_410() {
    let t = app().await;
    let token = "tok_ghost";
    let hash = lets_chat::auth::hash_api_token(&SECRET, token);
    db::room_feeds::insert(&t.chat, t.room, "rss", &hash, "ghost-no-such-user")
        .await
        .unwrap();
    assert_eq!(
        get_feed(&t.app, "rss", token, None).await.0,
        StatusCode::GONE
    );
}
