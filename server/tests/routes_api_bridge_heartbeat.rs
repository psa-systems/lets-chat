//! LC-78: bridge daemon heartbeat + sealed-config roundtrip + removal
//! preserves historical snapshot (stop-new lifecycle).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [13u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-hb-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    bot_id: String,
    admin_id: String,
    bridge_id: i64,
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
    let bot_id = db::auth::create_bot(&auth, "matrixbot").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bot_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "bridged", None, "public", None, Some(eid))
        .await
        .unwrap();
    // Insert via the real seal path so the roundtrip is exercised.
    let bridge_id = db::bridges::insert(
        &chat,
        &SECRET,
        room,
        "matrix",
        br#"{"homeserver":"https://matrix.org","secret":"abc"}"#,
        &bot_id,
        &admin,
    )
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
    };
    TestApp {
        app: routes::build_router(state),
        bot_id,
        admin_id: admin,
        bridge_id,
        chat,
        auth,
    }
}

async fn mint(t: &TestApp, user_id: &str, plaintext: &str, scopes: &str) {
    let hash = auth::hash_api_token(&SECRET, plaintext);
    db::api_tokens::insert(&t.auth, user_id, "tok", &hash, scopes, None)
        .await
        .unwrap();
}

async fn post(app: &Router, token: &str, uri: &str, json: &str) -> (StatusCode, Value) {
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
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

#[tokio::test]
async fn config_seal_roundtrip_under_secret_key() {
    let t = app().await;
    let plaintext = db::bridges::read_config(&t.chat, &SECRET, t.bridge_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        plaintext,
        br#"{"homeserver":"https://matrix.org","secret":"abc"}"#
    );
    // Different key cannot decrypt.
    let wrong_key = [0u8; 32];
    let res = db::bridges::read_config(&t.chat, &wrong_key, t.bridge_id).await;
    assert!(res.is_err(), "wrong key must not decrypt");
}

#[tokio::test]
async fn heartbeat_sets_healthy_and_updates_timestamp() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_hb_ok", "bridge:heartbeat").await;
    let (status, body) = post(
        &t.app,
        "lc_hb_ok",
        &format!("/api/v1/bridges/{}/heartbeat", t.bridge_id),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:?}");
    assert_eq!(body.get("status").unwrap().as_str().unwrap(), "healthy");
    let row: (Option<String>, String, Option<String>) =
        sqlx::query_as("SELECT last_heartbeat_at, status, last_error FROM bridges WHERE id = ?")
            .bind(t.bridge_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert!(row.0.is_some(), "last_heartbeat_at recorded");
    assert_eq!(row.1, "healthy");
    assert!(row.2.is_none(), "last_error cleared");
}

#[tokio::test]
async fn heartbeat_with_error_records_errored_status() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_hb_err", "bridge:heartbeat").await;
    let (status, body) = post(
        &t.app,
        "lc_hb_err",
        &format!("/api/v1/bridges/{}/heartbeat", t.bridge_id),
        r#"{"error":"homeserver unreachable"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("status").unwrap().as_str().unwrap(), "errored");
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, last_error FROM bridges WHERE id = ?")
            .bind(t.bridge_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(row.0, "errored");
    assert_eq!(row.1.as_deref(), Some("homeserver unreachable"));
}

#[tokio::test]
async fn heartbeat_recovers_from_error_on_next_clean_ping() {
    // A daemon that recovers from an error should not have to also tell us
    // to clear last_error; a subsequent error-free heartbeat is enough.
    let t = app().await;
    mint(&t, &t.bot_id, "lc_hb_rec", "bridge:heartbeat").await;
    let uri = format!("/api/v1/bridges/{}/heartbeat", t.bridge_id);
    post(&t.app, "lc_hb_rec", &uri, r#"{"error":"x"}"#).await;
    post(&t.app, "lc_hb_rec", &uri, "{}").await;
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, last_error FROM bridges WHERE id = ?")
            .bind(t.bridge_id)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(row.0, "healthy");
    assert!(row.1.is_none());
}

#[tokio::test]
async fn heartbeat_without_scope_is_403() {
    let t = app().await;
    // A bridge:post token cannot heartbeat. The two scopes are split so
    // an operator can give a daemon a heartbeat-only token if they want
    // read-side and posting in separate tokens.
    mint(&t, &t.bot_id, "lc_hb_no", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_hb_no",
        &format!("/api/v1/bridges/{}/heartbeat", t.bridge_id),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn heartbeat_from_wrong_bot_is_403() {
    let t = app().await;
    let other = db::auth::create_bot(&t.auth, "otherbot").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&other)
        .execute(&t.auth)
        .await
        .unwrap();
    mint(&t, &other, "lc_hb_other", "bridge:heartbeat").await;
    let (status, _) = post(
        &t.app,
        "lc_hb_other",
        &format!("/api/v1/bridges/{}/heartbeat", t.bridge_id),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn heartbeat_for_nonexistent_bridge_is_404() {
    let t = app().await;
    mint(&t, &t.bot_id, "lc_hb_404", "bridge:heartbeat").await;
    let (status, _) = post(&t.app, "lc_hb_404", "/api/v1/bridges/99999/heartbeat", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remove_bridge_preserves_historical_message_snapshot() {
    // STOP-NEW lifecycle (chunk 1 design decision, criterion-owner-deferred):
    // removing a bridge must NOT erase historical bridge messages. The FK is
    // ON DELETE SET NULL, and the row-snapshotted bridge_foreign_name +
    // bridge_kind columns persist so the render still surfaces the foreign
    // actor identity. This is the load-bearing test for that choice.
    let t = app().await;
    // Post a bridge message first.
    mint(&t, &t.bot_id, "lc_post", "bridge:post").await;
    let (status, _) = post(
        &t.app,
        "lc_post",
        &format!("/api/v1/bridges/{}/messages", t.bridge_id),
        r#"{"body":"hello from matrix","foreign_name":"alice:matrix.org"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Sanity: row has bridge_id set.
    let pre: (Option<i64>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT bridge_id, bridge_foreign_name, bridge_kind FROM messages \
         WHERE body = 'hello from matrix'",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert_eq!(pre.0, Some(t.bridge_id));
    assert_eq!(pre.1.as_deref(), Some("alice:matrix.org"));
    assert_eq!(pre.2.as_deref(), Some("matrix"));
    // Now remove the bridge.
    let removed = db::bridges::remove(&t.chat, t.bridge_id).await.unwrap();
    assert!(removed);
    // bridge_id is now NULL via ON DELETE SET NULL; snapshot strings persist.
    let post: (Option<i64>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT bridge_id, bridge_foreign_name, bridge_kind FROM messages \
         WHERE body = 'hello from matrix'",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    assert!(post.0.is_none(), "bridge_id nulled on removal");
    assert_eq!(
        post.1.as_deref(),
        Some("alice:matrix.org"),
        "snapshot name kept"
    );
    assert_eq!(post.2.as_deref(), Some("matrix"), "snapshot kind kept");
    // Also keeps the row reachable to admin promotion as the bridge author
    // (admin_id is the creator; this just confirms the row still exists).
    let _ = t.admin_id;
}
