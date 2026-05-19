//! LC-87 integration: per-room Files tab.
use axum::body::{to_bytes, Body};
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
            let p = std::env::temp_dir().join(format!("lc-files-tests-{}", std::process::id()));
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
    outsider_session: String,
    admin_id: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let outsider_id = db::auth::create_user(&auth, "outsider", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&outsider_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider_id).await.unwrap();
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
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        outsider_session,
        admin_id,
        chat: chat_for_test,
    }
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Helper: insert a message in room 1 + an upload row linked to it.
async fn seed_upload(
    chat: &SqlitePool,
    uploader: &str,
    filename: &str,
    mime: &str,
    size: i64,
) -> i64 {
    let mid: i64 = sqlx::query_scalar(
        "INSERT INTO messages (room_id, user_id, body) VALUES (1, ?, ?) RETURNING id",
    )
    .bind(uploader)
    .bind("see attached")
    .fetch_one(chat)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO file_uploads (uploader_id, message_id, filename, mime_type, size_bytes, storage_path) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uploader)
    .bind(mid)
    .bind(filename)
    .bind(mime)
    .bind(size)
    .bind(format!("/fake/{filename}"))
    .execute(chat)
    .await
    .unwrap();
    mid
}

#[tokio::test]
async fn empty_room_shows_no_files_notice() {
    let t = app().await;
    let (status, body) = get(&t.app, &t.admin_session, "/room/1/info?tab=files").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Files"), "tab nav present");
    assert!(
        body.contains("No files uploaded in this room yet."),
        "empty notice"
    );
    let _ = t.admin_id;
}

#[tokio::test]
async fn files_appear_in_tab() {
    let t = app().await;
    seed_upload(&t.chat, &t.admin_id, "photo.png", "image/png", 1234).await;
    seed_upload(&t.chat, &t.admin_id, "report.pdf", "application/pdf", 9999).await;
    let (status, body) = get(&t.app, &t.admin_session, "/room/1/info?tab=files").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("photo.png"), "image row visible");
    assert!(body.contains("report.pdf"), "pdf row visible");
}

#[tokio::test]
async fn filter_narrows_to_images_only() {
    let t = app().await;
    seed_upload(&t.chat, &t.admin_id, "photo.png", "image/png", 1234).await;
    seed_upload(&t.chat, &t.admin_id, "report.pdf", "application/pdf", 9999).await;
    let (status, body) = get(&t.app, &t.admin_session, "/room/1/files?kind=image").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("photo.png"));
    assert!(!body.contains("report.pdf"), "pdf filtered out: {body}");
}

#[tokio::test]
async fn deleted_messages_hide_files() {
    let t = app().await;
    let mid = seed_upload(&t.chat, &t.admin_id, "ghost.txt", "text/plain", 10).await;
    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id=?")
        .bind(mid)
        .execute(&t.chat)
        .await
        .unwrap();
    let (status, body) = get(&t.app, &t.admin_session, "/room/1/files?kind=all").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("ghost.txt"),
        "soft-deleted file leaked: {body}"
    );
}

#[tokio::test]
async fn outsider_cannot_list_private_room_files() {
    let t = app().await;
    // Make a brand new private room outsider is not in.
    let room_id: i64 = sqlx::query_scalar(
        "INSERT INTO rooms (name, room_type, enclave_id) VALUES ('secret', 'private', 1) RETURNING id",
    )
    .fetch_one(&t.chat)
    .await
    .unwrap();
    let uri = format!("/room/{room_id}/files");
    let (status, _) = get(&t.app, &t.outsider_session, &uri).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pagination_load_more_advances_cursor() {
    let t = app().await;
    // Seed enough uploads to span two pages (page size 40, so seed 45).
    for i in 0..45 {
        seed_upload(
            &t.chat,
            &t.admin_id,
            &format!("file{i}.txt"),
            "text/plain",
            100,
        )
        .await;
    }
    let (status, body) = get(&t.app, &t.admin_session, "/room/1/info?tab=files").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Load more"), "load-more button present");
    // The newest 40 should be visible.
    assert!(body.contains("file44.txt"));
    assert!(body.contains("file5.txt"));
    // file0..file4 should be on the next page only.
    assert!(!body.contains("file0.txt"), "page 1 leaked older rows");

    // Fetch page 2 via the fragment endpoint. Page 1 returns the
    // newest 40 (upload ids 6..45 = file5.txt..file44.txt); the load-
    // more cursor is the smallest id on page 1 (6). Querying with
    // `before=6` returns ids 5, 4, 3, 2, 1 = file4.txt..file0.txt.
    let (status, body2) = get(
        &t.app,
        &t.admin_session,
        "/room/1/files?kind=all&before=6&append=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body2.contains("file4.txt"),
        "page 2 has older rows: {body2}"
    );
    assert!(body2.contains("file0.txt"));
}
