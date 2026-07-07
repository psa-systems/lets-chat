//! LC-551 integration: graduated member trust.
//!
//! Covers the new-member posting cooldown over HTTP (a freshly-joined member's
//! rapid second post is refused; the owner is exempt) and the graduation +
//! manual-trust db logic (a new member becomes trusted after posting enough;
//! owners are never "new").

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-trust-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app() -> (TestApp, String, String) {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
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
    db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    (
        TestApp {
            app: routes::build_router(state),
            auth,
            chat,
        },
        alice,
        alice_session,
    )
}

async fn enclave_id(t: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT id FROM enclaves WHERE name = 'Acme'")
        .fetch_one(&t.chat)
        .await
        .unwrap()
}

async fn make_room(t: &TestApp, eid: i64) -> i64 {
    db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap()
}

async fn post(t: &TestApp, session: &str, room: i64, body: &str) -> StatusCode {
    let form = format!("body={body}");
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room}/messages"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(form))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let _ = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    status
}

#[tokio::test]
async fn new_member_rapid_second_post_is_rate_limited() {
    let (t, _alice, _alice_session) = app().await;
    let eid = enclave_id(&t).await;
    let room = make_room(&t, eid).await;

    // Bob joins as a plain member -> trust defaults to "new".
    let bob = db::auth::create_user(&t.auth, "bob", "h").await.unwrap();
    let bob_session = db::auth::create_session(&t.auth, &bob).await.unwrap();
    db::enclave::add_member(&t.chat, eid, &bob, EnclaveRole::Member)
        .await
        .unwrap();
    assert!(db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());

    // First post lands; the immediate second trips the new-member cooldown.
    assert_eq!(
        post(&t, &bob_session, room, "hi").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(&t, &bob_session, room, "again").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn owner_is_exempt_from_new_member_cooldown() {
    let (t, _alice, alice_session) = app().await;
    let eid = enclave_id(&t).await;
    let room = make_room(&t, eid).await;
    // The owner is never "new"; two rapid posts both land.
    assert_eq!(
        post(&t, &alice_session, room, "one").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(&t, &alice_session, room, "two").await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn member_graduates_after_threshold() {
    let (t, _alice, _s) = app().await;
    let eid = enclave_id(&t).await;
    let room = make_room(&t, eid).await;
    let bob = db::auth::create_user(&t.auth, "bob", "h").await.unwrap();
    db::enclave::add_member(&t.chat, eid, &bob, EnclaveRole::Member)
        .await
        .unwrap();

    // Below threshold: still new, no graduation.
    for _ in 0..(db::enclave::GRADUATE_AFTER_POSTS - 1) {
        db::chat::insert_message(&t.chat, room, &bob, "x")
            .await
            .unwrap();
    }
    assert!(!db::enclave::maybe_graduate(&t.chat, eid, &bob)
        .await
        .unwrap());
    assert!(db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());

    // Reaching the threshold graduates them exactly once.
    db::chat::insert_message(&t.chat, room, &bob, "x")
        .await
        .unwrap();
    assert!(db::enclave::maybe_graduate(&t.chat, eid, &bob)
        .await
        .unwrap());
    assert!(!db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());
    assert!(!db::enclave::maybe_graduate(&t.chat, eid, &bob)
        .await
        .unwrap());
}

#[tokio::test]
async fn manual_trust_lifts_the_new_flag() {
    let (t, _alice, _s) = app().await;
    let eid = enclave_id(&t).await;
    let bob = db::auth::create_user(&t.auth, "bob", "h").await.unwrap();
    db::enclave::add_member(&t.chat, eid, &bob, EnclaveRole::Member)
        .await
        .unwrap();
    assert!(db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());
    db::enclave::set_trust(&t.chat, eid, &bob, true)
        .await
        .unwrap();
    assert!(!db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());
    // And resettable back to new.
    db::enclave::set_trust(&t.chat, eid, &bob, false)
        .await
        .unwrap();
    assert!(db::enclave::is_new_member(&t.chat, eid, &bob)
        .await
        .unwrap());
}
