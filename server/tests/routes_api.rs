//! LC-72: scoped bearer API. Covers no-token 401, scope 403, expiry 401,
//! immediate revoke 401, and the read/write message endpoints.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [7u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-api-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice: String,
    room: i64,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&alice)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &alice)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat,
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
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        alice,
        room,
        auth,
    }
}

/// Mint a token with the given scopes + optional expiry, returning the
/// plaintext to present as a bearer.
async fn mint(t: &TestApp, plaintext: &str, scopes: &str, expires_at: Option<&str>) {
    let hash = auth::hash_api_token(&SECRET, plaintext);
    db::api_tokens::insert(&t.auth, &t.alice, "tok", &hash, scopes, expires_at)
        .await
        .unwrap();
}

async fn api_get(app: &Router, token: Option<&str>, uri: &str) -> (StatusCode, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(tk) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {tk}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn api_post(app: &Router, token: &str, uri: &str, json: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn no_or_bad_token_is_401() {
    let t = app().await;
    assert_eq!(
        api_get(&t.app, None, "/api/v1/me").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        api_get(&t.app, Some("lc_bogus"), "/api/v1/me").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn valid_token_resolves_identity() {
    let t = app().await;
    mint(&t, "lc_me", "rooms:read", None).await;
    let (status, body) = api_get(&t.app, Some("lc_me"), "/api/v1/me").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"username\":\"alice\""));
}

#[tokio::test]
async fn bearer_scheme_is_case_insensitive() {
    let t = app().await;
    mint(&t, "lc_lc", "rooms:read", None).await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/me")
        .header(header::AUTHORIZATION, "bearer lc_lc") // lowercase scheme
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn missing_scope_is_403_not_401() {
    let t = app().await;
    mint(&t, "lc_ro", "rooms:read", None).await;
    // rooms:read is allowed...
    assert_eq!(
        api_get(&t.app, Some("lc_ro"), "/api/v1/rooms").await.0,
        StatusCode::OK
    );
    // ...but writing requires messages:write -> 403, not 401.
    let (status, _) = api_post(
        &t.app,
        "lc_ro",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"hi\"}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn write_scope_posts_and_read_scope_reads() {
    let t = app().await;
    mint(&t, "lc_rw", "messages:read messages:write", None).await;
    let (status, _) = api_post(
        &t.app,
        "lc_rw",
        &format!("/api/v1/rooms/{}/messages", t.room),
        "{\"body\":\"hello from a bot\"}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = api_get(
        &t.app,
        Some("lc_rw"),
        &format!("/api/v1/rooms/{}/messages", t.room),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hello from a bot"));
}

#[tokio::test]
async fn api_message_over_length_cap_returns_400() {
    // LC-153: the bearer-token POST enforces the same 16k-char cap as the web
    // composer, so the API is not an unbounded-body amplification path.
    let t = app().await;
    mint(&t, "lc_big", "messages:write", None).await;
    let huge = "x".repeat(16_001);
    let (status, _) = api_post(
        &t.app,
        "lc_big",
        &format!("/api/v1/rooms/{}/messages", t.room),
        &format!("{{\"body\":\"{huge}\"}}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expired_token_is_401() {
    let t = app().await;
    mint(&t, "lc_old", "rooms:read", Some("2000-01-01 00:00:00")).await;
    assert_eq!(
        api_get(&t.app, Some("lc_old"), "/api/v1/me").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn revoked_token_is_401_immediately() {
    let t = app().await;
    mint(&t, "lc_rev", "rooms:read", None).await;
    assert_eq!(
        api_get(&t.app, Some("lc_rev"), "/api/v1/me").await.0,
        StatusCode::OK
    );
    // Revoke and retry: rejected on the next request.
    let hash = auth::hash_api_token(&SECRET, "lc_rev");
    let row = db::api_tokens::find_by_hash(&t.auth, &hash)
        .await
        .unwrap()
        .unwrap();
    assert!(db::api_tokens::revoke(&t.auth, row.id, &t.alice)
        .await
        .unwrap());
    assert_eq!(
        api_get(&t.app, Some("lc_rev"), "/api/v1/me").await.0,
        StatusCode::UNAUTHORIZED
    );
}
