//! LC-78: cursor pagination on GET /api/v1/rooms/{id}/messages.
//!
//! Pre-LC-78 the API returned every top-level message in the room unbounded;
//! the bridge daemon's initial-sync needs a deterministic bounded contract.
//! This file is also the first test coverage for this endpoint (LC-72 shipped
//! it without one).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{auth, db, routes, state::AppState, ws::hub::Hub};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [11u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-pag-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    room: i64,
    chat: sqlx::SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
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
    let hash = auth::hash_api_token(&SECRET, "lc_read");
    db::api_tokens::insert(&auth, &alice, "tok", &hash, "messages:read", None)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
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
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        room,
        chat,
    }
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer lc_read")
        .body(Body::empty())
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

async fn seed(t: &TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::with_capacity(n);
    // No author needed for the row to land in `list_messages_paginated`; the
    // empty user_id matches the synthetic-actor shape (webhook/email).
    for i in 0..n {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO messages (room_id, user_id, body) VALUES (?, '', ?) RETURNING id",
        )
        .bind(t.room)
        .bind(format!("msg-{i:03}"))
        .fetch_one(&t.chat)
        .await
        .unwrap();
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn default_limit_caps_at_50_and_returns_cursor() {
    let t = app().await;
    let ids = seed(&t, 60).await;
    let (status, body) = get(&t.app, &format!("/api/v1/rooms/{}/messages", t.room)).await;
    assert_eq!(status, StatusCode::OK);
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 50, "default limit is 50");
    // DESC order: first row is the highest id.
    assert_eq!(
        messages[0].get("id").unwrap().as_i64().unwrap(),
        *ids.last().unwrap()
    );
    // 60 seeded, 50 returned, so next_cursor is the oldest of THIS page.
    let cursor = body.get("next_cursor").unwrap().as_i64().unwrap();
    assert_eq!(cursor, ids[60 - 50]);
}

#[tokio::test]
async fn cursor_walks_to_history() {
    let t = app().await;
    let _ids = seed(&t, 7).await;
    let (_, first) = get(
        &t.app,
        &format!("/api/v1/rooms/{}/messages?limit=3", t.room),
    )
    .await;
    let first_msgs = first.get("messages").unwrap().as_array().unwrap();
    assert_eq!(first_msgs.len(), 3);
    let cursor = first.get("next_cursor").unwrap().as_i64().unwrap();
    let (_, second) = get(
        &t.app,
        &format!(
            "/api/v1/rooms/{}/messages?limit=3&before_id={cursor}",
            t.room
        ),
    )
    .await;
    let second_msgs = second.get("messages").unwrap().as_array().unwrap();
    assert_eq!(second_msgs.len(), 3);
    // Second page has strictly older ids than the first.
    let oldest_first = first_msgs[2].get("id").unwrap().as_i64().unwrap();
    let newest_second = second_msgs[0].get("id").unwrap().as_i64().unwrap();
    assert!(
        newest_second < oldest_first,
        "second page must come strictly before first"
    );
}

#[tokio::test]
async fn exhaust_returns_null_cursor() {
    let t = app().await;
    seed(&t, 3).await;
    let (status, body) = get(
        &t.app,
        &format!("/api/v1/rooms/{}/messages?limit=10", t.room),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert!(
        body.get("next_cursor").unwrap().is_null(),
        "exhausting history returns null cursor"
    );
}

#[tokio::test]
async fn empty_room_returns_empty_page_null_cursor() {
    let t = app().await;
    let (status, body) = get(&t.app, &format!("/api/v1/rooms/{}/messages", t.room)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("messages").unwrap().as_array().unwrap().is_empty());
    assert!(body.get("next_cursor").unwrap().is_null());
}

#[tokio::test]
async fn limit_is_clamped_to_max() {
    // limit=99999 must not return more than PAGINATED_MAX_LIMIT (200) rows.
    let t = app().await;
    seed(&t, 250).await;
    let (status, body) = get(
        &t.app,
        &format!("/api/v1/rooms/{}/messages?limit=99999", t.room),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 200, "limit clamped to 200");
}

#[tokio::test]
async fn bridge_message_uses_snapshot_author_after_removal() {
    // Stop-new lifecycle: when a bridge is removed, ON DELETE SET NULL nulls
    // out `bridge_id` on its historical messages while the snapshot strings
    // (foreign_name, kind) persist. The read API must still surface the
    // foreign_name as `author`, not fall back to a join that no longer
    // resolves. This is the load-bearing test for the "remove bridge does
    // not erase history" decision (chunk 1's stop-new vs delete-history call).
    let t = app().await;
    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, bridge_id, bridge_foreign_name, bridge_kind) \
         VALUES (?, '', 'from matrix', NULL, 'alice:matrix.org', 'matrix')",
    )
    .bind(t.room)
    .execute(&t.chat)
    .await
    .unwrap();
    let (status, body) = get(&t.app, &format!("/api/v1/rooms/{}/messages", t.room)).await;
    assert_eq!(status, StatusCode::OK);
    let messages = body.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].get("author").unwrap().as_str().unwrap(),
        "alice:matrix.org"
    );
}
