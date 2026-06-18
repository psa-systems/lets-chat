//! LC-93 integration: per-user and per-enclave storage quotas.
//!
//! Covers the DB helpers (set/get round-trip, usage SUMs that exclude
//! system messages, enclave aggregation across rooms), the upload
//! handler's user-quota 413, the admin form's audit-log + persistence,
//! and the message-create handler's enclave-quota 413 when an
//! otherwise-valid attachment would push the enclave over its cap.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-quota-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

fn tiny_png() -> Vec<u8> {
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    buf
}

fn multipart_body(field: &str, filename: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----lc-quota-boundary";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let ctype = format!("multipart/form-data; boundary={boundary}");
    (ctype, body)
}

async fn post_upload(app: &Router, session: &str, bytes: &[u8]) -> (StatusCode, String) {
    let (ctype, body) = multipart_body("file", "f.png", bytes);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/upload")
        .header(header::CONTENT_TYPE, ctype)
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
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
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// `admin_session`, `admin_id`, and `auth` are unused under the saas
// build because the admin-route tests that consume them are
// `#[cfg(feature = "standalone")]`. Allowing dead code keeps a single
// struct definition for both builds.
#[allow(dead_code)]
struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    member_id: String,
    auth: SqlitePool,
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        member_id,
        auth,
        chat,
    }
}

#[tokio::test]
async fn user_quota_round_trip_and_clear() {
    let t = app().await;
    assert_eq!(
        db::quota::get_user_quota(&t.chat, &t.member_id)
            .await
            .unwrap(),
        None
    );
    db::quota::set_user_quota(&t.chat, &t.member_id, Some(1024))
        .await
        .unwrap();
    assert_eq!(
        db::quota::get_user_quota(&t.chat, &t.member_id)
            .await
            .unwrap(),
        Some(1024)
    );
    db::quota::set_user_quota(&t.chat, &t.member_id, None)
        .await
        .unwrap();
    assert_eq!(
        db::quota::get_user_quota(&t.chat, &t.member_id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn enclave_quota_round_trip_and_clear() {
    let t = app().await;
    // The General enclave (id=1) exists after backfill.
    let id = 1i64;
    assert_eq!(
        db::quota::get_enclave_quota(&t.chat, id).await.unwrap(),
        None
    );
    db::quota::set_enclave_quota(&t.chat, id, Some(4096))
        .await
        .unwrap();
    assert_eq!(
        db::quota::get_enclave_quota(&t.chat, id).await.unwrap(),
        Some(4096)
    );
    db::quota::set_enclave_quota(&t.chat, id, None)
        .await
        .unwrap();
    assert_eq!(
        db::quota::get_enclave_quota(&t.chat, id).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn user_usage_excludes_system_messages_and_counts_orphans() {
    let t = app().await;
    // Orphan upload: counts toward user usage.
    let orphan_id = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "o.png",
        "image/png",
        100,
        "orphan.png",
        None,
    )
    .await
    .unwrap();
    let _ = orphan_id;
    assert_eq!(
        db::quota::sum_user_usage(&t.chat, &t.member_id)
            .await
            .unwrap(),
        100
    );

    // Attached to a system message: NOT counted.
    let sys_msg_id = db::chat::insert_system_message(&t.chat, 1, &t.member_id, "auto-thing")
        .await
        .unwrap();
    let sys_upload = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "s.png",
        "image/png",
        50,
        "sys.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, sys_upload, sys_msg_id)
        .await
        .unwrap();
    assert_eq!(
        db::quota::sum_user_usage(&t.chat, &t.member_id)
            .await
            .unwrap(),
        100,
        "system-attached upload should not count toward user usage",
    );

    // Attached to a normal message: counts.
    let real_msg_id = db::chat::insert_message(&t.chat, 1, &t.member_id, "hi")
        .await
        .unwrap();
    let real_upload = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "r.png",
        "image/png",
        25,
        "real.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, real_upload, real_msg_id)
        .await
        .unwrap();
    assert_eq!(
        db::quota::sum_user_usage(&t.chat, &t.member_id)
            .await
            .unwrap(),
        125
    );
}

#[tokio::test]
async fn soft_deleted_message_still_counts_against_both_quotas() {
    // Closes the self-delete-to-bypass-cap loophole: a user who
    // soft-deletes their own message must NOT free quota headroom,
    // because the upload row + bytes-on-disk are still there.
    let t = app().await;
    let mid = db::chat::insert_message(&t.chat, 1, &t.member_id, "hi")
        .await
        .unwrap();
    let up = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "d.png",
        "image/png",
        300,
        "d.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, up, mid)
        .await
        .unwrap();
    assert_eq!(
        db::quota::sum_user_usage(&t.chat, &t.member_id)
            .await
            .unwrap(),
        300
    );
    assert_eq!(db::quota::sum_enclave_usage(&t.chat, 1).await.unwrap(), 300);

    db::moderation::soft_delete_message(&t.chat, mid, &t.member_id)
        .await
        .unwrap();
    assert_eq!(
        db::quota::sum_user_usage(&t.chat, &t.member_id)
            .await
            .unwrap(),
        300,
        "soft-delete must not free user quota"
    );
    assert_eq!(
        db::quota::sum_enclave_usage(&t.chat, 1).await.unwrap(),
        300,
        "soft-delete must not free enclave quota"
    );
}

#[tokio::test]
async fn enclave_usage_joins_through_messages_and_rooms() {
    let t = app().await;
    // General enclave (id=1), General room (id=1).
    let msg = db::chat::insert_message(&t.chat, 1, &t.member_id, "hi")
        .await
        .unwrap();
    let up = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "a.png",
        "image/png",
        200,
        "e.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, up, msg)
        .await
        .unwrap();
    assert_eq!(db::quota::sum_enclave_usage(&t.chat, 1).await.unwrap(), 200);
}

#[tokio::test]
async fn upload_over_user_quota_returns_413() {
    let t = app().await;
    // Cap at 1 byte: even the smallest 1x1 PNG (~80 bytes) blows it.
    db::quota::set_user_quota(&t.chat, &t.member_id, Some(1))
        .await
        .unwrap();
    let (status, body) = post_upload(&t.app, &t.member_session, &tiny_png()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "body: {body}");
    assert!(
        body.contains("quota"),
        "413 body should mention quota; got: {body}"
    );
}

#[tokio::test]
async fn upload_under_user_quota_succeeds() {
    let t = app().await;
    db::quota::set_user_quota(&t.chat, &t.member_id, Some(10 * 1024))
        .await
        .unwrap();
    let (status, body) = post_upload(&t.app, &t.member_session, &tiny_png()).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}

// LC-177: the admin room list renders rows inside the swappable #room-{id}
// region the OOB fragment targets, and page-rendered rows are NOT OOB
// (oob=false on the HTTP path). The admin topic subscription is shared with the
// user list via admin_layout.
#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_room_list_is_wired_for_live_row_updates() {
    let t = app().await;
    let rid = db::chat::create_room(&t.chat, "liveroom", None, "public", None, None)
        .await
        .unwrap();
    let (status, body) = send(&t.app, &t.admin_session, Method::GET, "/admin/rooms", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-live-topic=\"admin\""),
        "admin pages subscribe to the admin topic",
    );
    assert!(
        body.contains(&format!("id=\"room-{rid}\"")),
        "room list renders the OOB-targetable row id",
    );
    assert!(
        !body.contains("hx-swap-oob"),
        "page-rendered rows must not be OOB (only the live WS row is)",
    );
}

// LC-175: the admin user list subscribes to the `admin` topic so ban / mute /
// role / quota / delete refresh the row live across all admins. Pin the
// subscription, the member row id (the OOB target), and that the page-rendered
// row is NOT itself OOB (oob=false on the HTTP path).
#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_user_list_is_wired_for_live_row_updates() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.admin_session, Method::GET, "/admin/users", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-live-topic=\"admin\""),
        "admin pages must subscribe to the admin topic for live row updates",
    );
    assert!(
        body.contains(&format!("id=\"user-{}\"", t.member_id)),
        "user list renders the OOB-targetable row id",
    );
    assert!(
        !body.contains("hx-swap-oob"),
        "page-rendered rows must not be OOB (only the live WS row is)",
    );
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_form_persists_user_quota_and_writes_audit_row() {
    let t = app().await;
    let (status, _body) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/admin/users/{}/quota", t.member_id),
        "quota_mib=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "HTMX row replace returns 200");

    let stored = db::quota::get_user_quota(&t.chat, &t.member_id)
        .await
        .unwrap();
    assert_eq!(stored, Some(5 * 1024 * 1024));

    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let q = actions
        .iter()
        .find(|a| a.action == "quota_set_user")
        .expect("audit row recorded");
    assert_eq!(q.actor_user, t.admin_id);
    assert_eq!(q.target_user, t.member_id);
    assert_eq!(
        q.metadata.as_deref(),
        Some((5 * 1024 * 1024).to_string().as_str())
    );

    // Clearing the quota (empty input) deletes the row + logs "unlimited".
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        &format!("/admin/users/{}/quota", t.member_id),
        "quota_mib=",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        db::quota::get_user_quota(&t.chat, &t.member_id)
            .await
            .unwrap(),
        None
    );
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert!(
        actions
            .iter()
            .any(|a| a.action == "quota_set_user" && a.metadata.as_deref() == Some("unlimited")),
        "clearing should log unlimited"
    );
    // Suppress unused-field warnings: `auth` is held by the harness so
    // the bg-writer doesn't observe a dropped pool mid-test.
    let _ = &t.auth;
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_form_persists_enclave_quota_and_writes_audit_row() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/admin/enclaves/1/quota",
        "quota_mib=3",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    assert_eq!(
        db::quota::get_enclave_quota(&t.chat, 1).await.unwrap(),
        Some(3 * 1024 * 1024)
    );
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let q = actions
        .iter()
        .find(|a| a.action == "quota_set_enclave")
        .expect("audit row recorded");
    assert_eq!(q.actor_user, t.admin_id);
    assert_eq!(q.room_id, Some(1));
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn non_admin_cannot_set_user_quota() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.member_session,
        Method::POST,
        &format!("/admin/users/{}/quota", t.member_id),
        "quota_mib=5",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_quota_form_404s_on_unknown_user_id() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/admin/users/nope-not-a-user/quota",
        "quota_mib=5",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        db::quota::get_user_quota(&t.chat, "nope-not-a-user")
            .await
            .unwrap(),
        None,
        "no orphan quota row should land for a non-existent user"
    );
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_quota_form_404s_on_unknown_enclave_id() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/admin/enclaves/9999/quota",
        "quota_mib=5",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert!(
        !actions.iter().any(|a| a.action == "quota_set_enclave"),
        "audit log must not record a no-op",
    );
}

#[tokio::test]
async fn message_attach_rejects_413_when_over_enclave_quota() {
    let t = app().await;
    // Pre-existing usage in the enclave: 1000 bytes.
    let msg = db::chat::insert_message(&t.chat, 1, &t.member_id, "prior")
        .await
        .unwrap();
    let up = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "p.png",
        "image/png",
        1000,
        "prior.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, up, msg)
        .await
        .unwrap();

    // Cap enclave at 1500 bytes - a fresh 600-byte upload would push to 1600.
    db::quota::set_enclave_quota(&t.chat, 1, Some(1500))
        .await
        .unwrap();

    // Orphan upload sized just over the headroom. We bypass the upload
    // handler so this test stays focused on the message-create gate.
    let new_up = db::uploads::insert_upload(
        &t.chat,
        &t.member_id,
        "n.png",
        "image/png",
        600,
        "new.png",
        None,
    )
    .await
    .unwrap();

    let body = format!("body=&file_id={new_up}");
    let (status, text) = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/messages",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "body: {text}");
    assert!(
        text.contains("enclave"),
        "413 body should mention enclave; got: {text}"
    );
}
