//! LC-304: a posted message containing another member's highlight word writes
//! a mention row for them (so the badge/highlight/notification reuse the
//! mention pipeline). Mirrors the routes_mentions harness: `viewer` (admin)
//! posts; `peer` holds the keyword.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-kw-mentions-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    viewer_session: String,
    peer_session: String,
    viewer_id: String,
    peer_id: String,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    let peer_id = db::auth::create_user(&auth, "peer", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&peer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    let peer_session = db::auth::create_session(&auth, &peer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
        viewer_session,
        peer_session,
        viewer_id,
        peer_id,
        chat,
        auth,
    }
}

async fn post_message(app: &Router, sess: &str, room_id: i64, body: &str) -> StatusCode {
    let form = format!("body={}&file_id=", body.replace(' ', "+"));
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn count_mentions_for_user(chat: &SqlitePool, user_id: &str) -> i64 {
    sqlx::query("SELECT COUNT(*) AS n FROM mentions WHERE mentioned_user_id = ?")
        .bind(user_id)
        .fetch_one(chat)
        .await
        .unwrap()
        .get::<i64, _>("n")
}

// POST a keyword-add for the given session.
async fn add_keyword(app: &Router, sess: &str, word: &str) {
    let form = format!("word={}", word.replace(' ', "+"));
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/keywords")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(form))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn keyword_match_inserts_mention_for_member() {
    let t = setup().await;
    add_keyword(&t.app, &t.peer_session, "deploy").await;
    // viewer (not peer) posts a message containing the peer's keyword.
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "please deploy now").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.peer_id).await,
        1,
        "keyword match should insert a mention row for the peer"
    );
}

#[tokio::test]
async fn keyword_match_is_case_insensitive() {
    let t = setup().await;
    add_keyword(&t.app, &t.peer_session, "deploy").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "DEPLOY the thing").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(count_mentions_for_user(&t.chat, &t.peer_id).await, 1);
}

#[tokio::test]
async fn keyword_respects_word_boundary() {
    let t = setup().await;
    add_keyword(&t.app, &t.peer_session, "deploy").await;
    // "deployment" must NOT match the word "deploy".
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "the deployment finished").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.peer_id).await,
        0,
        "deployment should not match the word deploy"
    );
}

#[tokio::test]
async fn author_does_not_self_notify_on_own_keyword() {
    let t = setup().await;
    // The viewer holds the keyword and posts it themselves.
    add_keyword(&t.app, &t.viewer_session, "urgent").await;
    assert_eq!(
        post_message(&t.app, &t.viewer_session, 1, "this is urgent").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.viewer_id).await,
        0,
        "author should not be mentioned by their own keyword"
    );
}

#[tokio::test]
async fn non_member_with_keyword_is_not_targeted() {
    let t = setup().await;
    // A private room only the viewer joins; the peer (keyword holder) is out.
    add_keyword(&t.app, &t.peer_session, "deploy").await;
    let private_room = db::chat::create_room(&t.chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&t.chat, private_room, &t.viewer_id)
        .await
        .unwrap();
    assert_eq!(
        post_message(&t.app, &t.viewer_session, private_room, "deploy now").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        count_mentions_for_user(&t.chat, &t.peer_id).await,
        0,
        "a non-member must not be keyword-targeted"
    );
    let _ = &t.auth;
}
