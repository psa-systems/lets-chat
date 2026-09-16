//! LC-902: the bearer-token API POST must refuse exactly what the web
//! composer refuses. Covers the send gates that `routes::api::post_message`
//! previously skipped: posting policy, DM block, enclave ban, rate limit,
//! and the link filter.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [7u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-api-gates-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    auth: SqlitePool,
    chat: SqlitePool,
    settings: SqlitePool,
    enclave_id: i64,
    room: i64,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth_pool = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let owner = db::auth::create_user(&auth_pool, "owner", "h")
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth_pool, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &owner)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "announcements", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth_pool.clone(),
        chat: chat.clone(),
        settings: settings.clone(),
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
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        auth: auth_pool,
        chat,
        settings,
        enclave_id: eid,
        room,
    }
}

/// Mint a token with the given scopes for `user_id`, returning the plaintext
/// to present as a bearer.
async fn mint(t: &TestApp, user_id: &str, plaintext: &str, scopes: &str) {
    let hash = auth::hash_api_token(&SECRET, plaintext);
    db::api_tokens::insert(&t.auth, user_id, "tok", &hash, scopes, None)
        .await
        .unwrap();
}

async fn api_post(
    app: &Router,
    token: &str,
    uri: &str,
    json: &str,
) -> (StatusCode, Vec<(String, String)>, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn message_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn admins_only_room_refuses_non_admin_token_owner() {
    let t = app().await;
    let bob = db::auth::create_user(&t.auth, "bob", "h").await.unwrap();
    db::enclave::add_member(&t.chat, t.enclave_id, &bob, EnclaveRole::Member)
        .await
        .unwrap();
    db::chat::set_room_posting_policy(&t.chat, t.room, "admins_only")
        .await
        .unwrap();
    mint(&t, &bob, "lc_bob", "messages:write").await;

    let (status, _, _) = api_post(
        &t.app,
        "lc_bob",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"sneaking in\"}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(message_count(&t.chat).await, 0);
}

#[tokio::test]
async fn dm_blocked_by_peer_refuses_send() {
    let t = app().await;
    let alice = db::auth::create_user(&t.auth, "alice", "h").await.unwrap();
    let carol = db::auth::create_user(&t.auth, "carol", "h").await.unwrap();
    let dm = db::chat::create_dm_room(&t.chat, "dm", &alice, &carol)
        .await
        .unwrap();
    // Carol blocks Alice; Alice (the token owner) then tries to post into the DM.
    db::auth::block_user(&t.auth, &carol, &alice).await.unwrap();
    mint(&t, &alice, "lc_alice", "messages:write").await;

    let (status, _, _) = api_post(
        &t.app,
        "lc_alice",
        &format!("/api/v1/rooms/{}/messages", dm.id),
        "{\"body\":\"hi\"}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(message_count(&t.chat).await, 0);
}

#[tokio::test]
async fn enclave_banned_user_refuses_send() {
    let t = app().await;
    let dave = db::auth::create_user(&t.auth, "dave", "h").await.unwrap();
    db::enclave::add_member(&t.chat, t.enclave_id, &dave, EnclaveRole::Member)
        .await
        .unwrap();
    db::enclave::ban_from_enclave(&t.chat, t.enclave_id, &dave, "spam")
        .await
        .unwrap();
    mint(&t, &dave, "lc_dave", "messages:write").await;

    let (status, _, _) = api_post(
        &t.app,
        "lc_dave",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"let me in\"}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(message_count(&t.chat).await, 0);
}

#[tokio::test]
async fn rate_limit_exceeded_returns_429_with_retry_after() {
    let t = app().await;
    let erin = db::auth::create_user(&t.auth, "erin", "h").await.unwrap();
    db::enclave::add_member(&t.chat, t.enclave_id, &erin, EnclaveRole::Member)
        .await
        .unwrap();
    db::settings::set_setting(&t.settings, "rate_limit_messages", "1")
        .await
        .unwrap();
    mint(&t, &erin, "lc_erin", "messages:write").await;

    let (status, _, _) = api_post(
        &t.app,
        "lc_erin",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"first\"}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, headers, _) = api_post(
        &t.app,
        "lc_erin",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"second\"}",
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("retry-after")),
        "missing Retry-After header: {headers:?}"
    );
    assert_eq!(message_count(&t.chat).await, 1);
}

#[tokio::test]
async fn blocked_link_host_refuses_send() {
    let t = app().await;
    let frank = db::auth::create_user(&t.auth, "frank", "h").await.unwrap();
    db::enclave::add_member(&t.chat, t.enclave_id, &frank, EnclaveRole::Member)
        .await
        .unwrap();
    db::settings::set_setting(&t.settings, "link_filter_enabled", "true")
        .await
        .unwrap();
    db::anti_spam::insert_rule(
        &t.chat,
        "evil.example.com",
        db::anti_spam::FilterAction::Block,
        &frank,
    )
    .await
    .unwrap();
    mint(&t, &frank, "lc_frank", "messages:write").await;

    let (status, _, _) = api_post(
        &t.app,
        "lc_frank",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"check http://evil.example.com/x\"}",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(message_count(&t.chat).await, 0);
}
