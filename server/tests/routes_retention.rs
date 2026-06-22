//! Integration tests for `routes::retention`.
//!
//! Covers the validation gate (1-day floor, DM rejection, non-admin
//! 403), the audit-log shape (`mod_actions` row with the right action /
//! target / metadata), the disable path (empty `days` clears the
//! column), and the preview fragment's count agreement with what the
//! sweep would actually delete (the load-bearing shared-predicate
//! invariant; the same property is also tested at the SQL level in
//! `tests/retention_sweep.rs::preview_count_equals_sweep_actual_delete`).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-retention-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
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
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        chat: chat_for_test,
    }
}

async fn send(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    (status, body)
}

async fn make_dm_room(chat: &SqlitePool) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES ('dm-pair', 'dm')")
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn admin_can_set_and_disable_retention_with_audit() {
    let t = app().await;

    // Enable.
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/retention",
        "days=30",
    )
    .await;
    assert!(
        status == StatusCode::SEE_OTHER || status == StatusCode::FOUND,
        "expected redirect, got {status}",
    );
    let days: Option<i64> = db::chat::get_room_retention_days(&t.chat, 1).await.unwrap();
    assert_eq!(days, Some(30));

    // Disable (empty days).
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/retention",
        "days=",
    )
    .await;
    assert!(status.is_redirection());
    let days: Option<i64> = db::chat::get_room_retention_days(&t.chat, 1).await.unwrap();
    assert_eq!(days, None);

    // Audit log has two retention_set rows. Newest-first ordering is
    // already provided by `list_mod_actions`.
    let rows = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let mine: Vec<_> = rows
        .iter()
        .filter(|r| r.action == "retention_set")
        .collect();
    assert_eq!(mine.len(), 2);
    // The most recent is the disable.
    assert_eq!(mine[0].target_user, "-");
    assert_eq!(mine[0].actor_user, t.admin_id);
    assert_eq!(mine[0].room_id, Some(1));
    let disable_meta = mine[0].metadata.as_deref().unwrap_or("");
    assert!(
        disable_meta.contains("\"old_days\":30") && disable_meta.contains("\"new_days\":null"),
        "disable audit metadata shape: {disable_meta}",
    );
    let enable_meta = mine[1].metadata.as_deref().unwrap_or("");
    assert!(
        enable_meta.contains("\"old_days\":null") && enable_meta.contains("\"new_days\":30"),
        "enable audit metadata shape: {enable_meta}",
    );
}

#[tokio::test]
async fn post_days_zero_is_rejected_with_400() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/retention",
        "days=0",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let days = db::chat::get_room_retention_days(&t.chat, 1).await.unwrap();
    assert_eq!(days, None, "rejected POST must not mutate state");
}

#[tokio::test]
async fn post_negative_days_is_rejected_with_400() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/retention",
        "days=-5",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_non_numeric_days_is_rejected_with_400() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/retention",
        "days=forever",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn non_admin_member_cannot_set_retention() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/retention",
        "days=30",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let days = db::chat::get_room_retention_days(&t.chat, 1).await.unwrap();
    assert_eq!(days, None);
}

#[tokio::test]
async fn dm_room_rejects_retention_post_with_400() {
    let t = app().await;
    let dm = make_dm_room(&t.chat).await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/room/{dm}/retention"),
        "days=30",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let days = db::chat::get_room_retention_days(&t.chat, dm)
        .await
        .unwrap();
    assert_eq!(days, None);
}

#[tokio::test]
async fn preview_fragment_includes_count_and_warnings() {
    let t = app().await;
    // Seed 3 stale messages in room 1.
    for i in 0..3 {
        let body = format!("stale-{i}");
        let id: i64 =
            sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (1, 'u1', ?)")
                .bind(&body)
                .execute(&t.chat)
                .await
                .unwrap()
                .last_insert_rowid();
        sqlx::query("UPDATE messages SET created_at = datetime('now', '-60 days') WHERE id = ?")
            .bind(id)
            .execute(&t.chat)
            .await
            .unwrap();
    }
    // One recent message that should NOT count.
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (1, 'u1', 'fresh')")
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body) = send(
        &t.app,
        &t.admin_session,
        Method::GET,
        "/room/1/retention/preview?days=30",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The fragment must surface the exact count (3), the permanence
    // language, and the no-pinned-exemption notice.
    assert!(body.contains(">3<"), "expected count 3 in fragment: {body}");
    assert!(
        body.contains("Permanent"),
        "expected permanence warning in fragment",
    );
    assert!(
        body.contains("Pinned messages are NOT exempt"),
        "expected pinned warning in fragment",
    );
    assert!(
        body.contains("active threads are preserved"),
        "expected loose-correct thread language in fragment",
    );
    assert!(
        body.contains("action=\"/room/1/retention\""),
        "expected embedded confirm form action",
    );
}

#[tokio::test]
async fn preview_with_days_zero_is_rejected_with_400() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::GET,
        "/room/1/retention/preview?days=0",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_with_empty_days_renders_disable_fragment() {
    let t = app().await;
    // Enable first so the disable preview has an `old_days` to show.
    let _ = db::chat::set_room_retention_days(&t.chat, 1, Some(30)).await;

    let (status, body) = send(
        &t.app,
        &t.admin_session,
        Method::GET,
        "/room/1/retention/preview?days=",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Disable retention"),
        "expected disable phrasing in fragment: {body}",
    );
    assert!(
        body.contains("previously-deleted messages will stay deleted"),
        "expected irreversibility notice in disable fragment",
    );
}

#[tokio::test]
async fn preview_on_dm_room_rejects_with_400() {
    let t = app().await;
    let dm = make_dm_room(&t.chat).await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::GET,
        &format!("/room/{dm}/retention/preview?days=30"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
