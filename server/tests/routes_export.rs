use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-export-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

struct TestApp {
    app: Router,
    viewer_id: String,
    viewer_session: String,
    other_id: String,
    auth: SqlitePool,
    chat: SqlitePool,
}

async fn app_with_two_users() -> TestApp {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, "viewer", "hash")
        .await
        .unwrap();
    let other_id = db::auth::create_user(&auth, "other", "hash").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id = ?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: None,
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
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_id,
        viewer_session,
        other_id,
        auth,
        chat,
    }
}

async fn send(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
) -> (StatusCode, Vec<u8>, http::HeaderMap) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 4 << 20).await.unwrap().to_vec();
    (status, bytes, headers)
}

#[tokio::test]
async fn export_anonymous_redirects_to_login() {
    let t = app_with_two_users().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/settings/export-data")
        .body(Body::empty())
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::SEE_OTHER
            || status == StatusCode::FOUND
            || status == StatusCode::TEMPORARY_REDIRECT
            || status == StatusCode::UNAUTHORIZED,
        "unexpected status: {status}"
    );
}

#[tokio::test]
async fn export_returns_json_attachment_with_user_data() {
    let t = app_with_two_users().await;

    // Seed a public room in the General enclave and add some viewer activity.
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let room_id = db::chat::create_room(
        &t.chat,
        "export-room",
        None,
        "public",
        None,
        Some(general_id),
    )
    .await
    .unwrap();
    let msg_a = db::chat::insert_message(&t.chat, room_id, &t.viewer_id, "hello world")
        .await
        .unwrap();
    let _msg_b = db::chat::insert_message(&t.chat, room_id, &t.other_id, "from someone else")
        .await
        .unwrap();
    // Viewer reacts to their own message and bookmarks it.
    sqlx::query("INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)")
        .bind(msg_a)
        .bind(&t.viewer_id)
        .bind("👍")
        .execute(&t.chat)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bookmarks (user_id, message_id) VALUES (?, ?)")
        .bind(&t.viewer_id)
        .bind(msg_a)
        .execute(&t.chat)
        .await
        .unwrap();
    // Viewer blocks the other user.
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (?, ?)")
        .bind(&t.viewer_id)
        .bind(&t.other_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (status, body, headers) = send(
        &t.app,
        &t.viewer_session,
        Method::GET,
        "/settings/export-data",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let disposition = headers
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(disposition.starts_with("attachment; filename=\""));
    assert!(disposition.contains("lets-chat-export-viewer-"));
    let content_type = headers.get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(content_type.starts_with("application/json"));

    let parsed: Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["profile"]["username"], "viewer");
    assert_eq!(parsed["profile"]["user_id"], t.viewer_id);

    let messages = parsed["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "only viewer's own message exported");
    assert_eq!(messages[0]["body"], "hello world");

    let reactions = parsed["reactions_given"]
        .as_array()
        .expect("reactions array");
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0]["emoji"], "👍");

    let bookmarks = parsed["bookmarks"].as_array().expect("bookmarks array");
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["message_id"], msg_a);

    let blocked = parsed["blocked_users"].as_array().expect("blocked array");
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["user_id"], t.other_id);

    let memberships = parsed["enclave_memberships"]
        .as_array()
        .expect("enclave_memberships array");
    assert!(
        memberships.iter().any(|m| m["enclave_name"] == "General"),
        "General membership missing: {memberships:?}"
    );

    let sessions = parsed["sessions"].as_array().expect("sessions array");
    assert!(!sessions.is_empty(), "session list empty");
    // The raw session id is the cookie bearer token; the export must
    // expose only its SHA-256, never the raw value.
    let raw_token = t.viewer_session.as_bytes();
    let s = &sessions[0];
    assert!(s.get("id").is_none(), "raw session id leaked: {s}");
    let hash = s["id_sha256"].as_str().expect("id_sha256 string");
    assert_eq!(hash.len(), 64, "id_sha256 should be 64 hex chars: {hash}");
    assert!(
        !body.windows(raw_token.len()).any(|w| w == raw_token),
        "raw session token appears in export body"
    );
}

#[tokio::test]
async fn export_scopes_data_to_the_caller() {
    // The viewer's export must NOT contain rows belonging to the other user.
    let t = app_with_two_users().await;
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let room_id = db::chat::create_room(
        &t.chat,
        "scope-room",
        None,
        "public",
        None,
        Some(general_id),
    )
    .await
    .unwrap();
    db::chat::insert_message(&t.chat, room_id, &t.other_id, "stranger says hi")
        .await
        .unwrap();
    sqlx::query("INSERT INTO bookmarks (user_id, message_id) VALUES (?, ?)")
        .bind(&t.other_id)
        .bind(1)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body, _) = send(
        &t.app,
        &t.viewer_session,
        Method::GET,
        "/settings/export-data",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed: Value = serde_json::from_slice(&body).unwrap();
    let messages = parsed["messages"].as_array().unwrap();
    assert!(
        messages.is_empty(),
        "viewer should have no messages, got: {messages:?}"
    );
    let bookmarks = parsed["bookmarks"].as_array().unwrap();
    assert!(bookmarks.is_empty(), "viewer should have no bookmarks");
}
