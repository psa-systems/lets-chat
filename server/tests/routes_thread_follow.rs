//! LC-310: thread following. Posting a reply auto-follows the replier AND the
//! thread root's author; the follow/unfollow endpoints flip state; a user who
//! never participates is not auto-followed.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-thread-follow-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

struct TestApp {
    app: Router,
    author_session: String,
    replier_session: String,
    author_id: String,
    replier_id: String,
    chat: sqlx::SqlitePool,
    root_id: i64,
}

async fn setup() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let author_id = db::auth::create_user(&auth, "author", "h").await.unwrap();
    let replier_id = db::auth::create_user(&auth, "replier", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&author_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&replier_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    // A thread root authored by `author` in the General room (id 1).
    let root_id = db::chat::insert_message(&chat, 1, &author_id, "root message")
        .await
        .unwrap();

    let author_session = db::auth::create_session(&auth, &author_id).await.unwrap();
    let replier_session = db::auth::create_session(&auth, &replier_id).await.unwrap();
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
    };
    TestApp {
        app: routes::build_router(state),
        author_session,
        replier_session,
        author_id,
        replier_id,
        chat,
        root_id,
    }
}

async fn post_reply(app: &Router, sess: &str, root_id: i64, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/1/thread/{root_id}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(format!("body={}", body.replace(' ', "+"))))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn toggle(app: &Router, sess: &str, root_id: i64, method: Method) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(format!("/room/1/thread/{root_id}/follow"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn replying_auto_follows_replier_and_root_author() {
    let t = setup().await;
    assert_eq!(
        post_reply(&t.app, &t.replier_session, t.root_id, "good point").await,
        StatusCode::NO_CONTENT
    );
    assert!(
        db::thread_followers::is_following(&t.chat, &t.replier_id, t.root_id)
            .await
            .unwrap(),
        "replier should be auto-followed"
    );
    assert!(
        db::thread_followers::is_following(&t.chat, &t.author_id, t.root_id)
            .await
            .unwrap(),
        "thread root author should be auto-followed"
    );
}

#[tokio::test]
async fn follow_and_unfollow_endpoints_flip_state() {
    let t = setup().await;
    // A user who has not replied is not following yet.
    assert!(
        !db::thread_followers::is_following(&t.chat, &t.replier_id, t.root_id)
            .await
            .unwrap()
    );

    let (status, body) = toggle(&t.app, &t.replier_session, t.root_id, Method::POST).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("room-thread") || body.contains("Following") || body.contains("hx-delete"),
        "expected following button: {body}"
    );
    assert!(
        db::thread_followers::is_following(&t.chat, &t.replier_id, t.root_id)
            .await
            .unwrap()
    );

    let (status, body) = toggle(&t.app, &t.replier_session, t.root_id, Method::DELETE).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("hx-post"),
        "expected follow button after unfollow: {body}"
    );
    assert!(
        !db::thread_followers::is_following(&t.chat, &t.replier_id, t.root_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn non_participant_is_not_auto_followed() {
    let t = setup().await;
    // author replies to their own thread; the replier never participates.
    assert_eq!(
        post_reply(&t.app, &t.author_session, t.root_id, "following up").await,
        StatusCode::NO_CONTENT
    );
    assert!(
        !db::thread_followers::is_following(&t.chat, &t.replier_id, t.root_id)
            .await
            .unwrap(),
        "a non-participant must not be auto-followed"
    );
}
