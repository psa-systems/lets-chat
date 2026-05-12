use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

/// PNG fixture bytes: 1x1 transparent PNG. The magic-byte sniffer (`infer`)
/// recognizes the PNG signature regardless of payload size.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// ZIP fixture bytes: empty ZIP archive (PK\x03\x04 header). Used to assert
/// that renaming a non-image to .png does not bypass magic-byte sniffing.
const TINY_ZIP: &[u8] = b"PK\x03\x04\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lets-chat-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
            include_str!("../migrations/auth/0007_notification_settings.sql"),
            include_str!("../migrations/auth/0008_two_factor.sql"),
            include_str!("../migrations/auth/0009_push_subscriptions.sql"),
            include_str!("../migrations/auth/0010_digest_columns.sql"),
        ],
        "chat" => vec![
            include_str!("../migrations/chat/0001_create_tables.sql"),
            include_str!("../migrations/chat/0002_moderation.sql"),
            include_str!("../migrations/chat/0003_dms.sql"),
            include_str!("../migrations/chat/0004_message_editing.sql"),
            include_str!("../migrations/chat/0005_private_rooms.sql"),
            include_str!("../migrations/chat/0006_read_receipts.sql"),
            include_str!("../migrations/chat/0007_reactions.sql"),
            include_str!("../migrations/chat/0008_search.sql"),
            include_str!("../migrations/chat/0009_enclaves.sql"),
            include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
            include_str!("../migrations/chat/0011_threads.sql"),
            include_str!("../migrations/chat/0012_uploads.sql"),
            include_str!("../migrations/chat/0013_link_previews.sql"),
            include_str!("../migrations/chat/0014_mentions.sql"),
            include_str!("../migrations/chat/0015_room_notification_settings.sql"),
            include_str!("../migrations/chat/0016_pinned_messages.sql"),
            include_str!("../migrations/chat/0017_custom_emojis.sql"),
            include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
            include_str!("../migrations/chat/0019_bookmarks.sql"),
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_smtp_settings.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn app_with_user(username: &str) -> (Router, String, String) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let user_id = db::auth::create_user(&auth, username, "hash")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session_token = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        email_client: None,
    };
    let app = routes::build_router(state);
    (app, session_token, user_id)
}

fn multipart_body(field: &str, filename: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----lc-test-boundary";
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

async fn post_upload(
    app: Router,
    session: &str,
    filename: &str,
    bytes: &[u8],
) -> (StatusCode, String) {
    let (ctype, body) = multipart_body("file", filename, bytes);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/upload")
        .header(header::CONTENT_TYPE, ctype)
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, text)
}

#[tokio::test]
async fn upload_happy_path_returns_file_id_and_url() {
    let (app, sess, _uid) = app_with_user("alice").await;
    let (status, body) = post_upload(app, &sess, "tiny.png", TINY_PNG).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(v["file_id"].is_i64());
    let id = v["file_id"].as_i64().unwrap();
    assert_eq!(v["url"], format!("/api/files/{id}"));
}

#[tokio::test]
async fn upload_oversized_returns_413() {
    let (app, sess, _uid) = app_with_user("bob").await;
    // 11 MiB > default 10 MiB cap.
    let big = vec![0u8; 11 * 1024 * 1024];
    let (status, _body) = post_upload(app, &sess, "huge.png", &big).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn upload_rejects_zip_renamed_as_png() {
    let (app, sess, _uid) = app_with_user("carol").await;
    let (status, _body) = post_upload(app, &sess, "decoy.png", TINY_ZIP).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn upload_anonymous_redirects_to_login() {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        email_client: None,
    };
    let app = routes::build_router(state);

    let (ctype, body) = multipart_body("file", "tiny.png", TINY_PNG);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/upload")
        .header(header::CONTENT_TYPE, ctype)
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // AuthUser extractor returns a 303 to /login when no session is present.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn file_serve_round_trips_uploaded_bytes() {
    let (app, sess, _uid) = app_with_user("dave").await;
    let (status, body) = post_upload(app.clone(), &sess, "tiny.png", TINY_PNG).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = v["file_id"].as_i64().unwrap();

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/files/{id}"))
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(ctype, "image/png");
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec();
    assert_eq!(bytes, TINY_PNG);
}

async fn app_with_two_users() -> (Router, String, String, String, String) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let id_a = db::auth::create_user(&auth, "alice3", "h").await.unwrap();
    let id_b = db::auth::create_user(&auth, "bob3", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&id_a)
        .bind(&id_b)
        .execute(&auth)
        .await
        .unwrap();
    let sess_a = db::auth::create_session(&auth, &id_a).await.unwrap();
    let sess_b = db::auth::create_session(&auth, &id_b).await.unwrap();
    // Both users need general-enclave membership before they can post in
    // any seeded public room. The backfill helper only runs when an admin
    // exists, so add memberships directly here.
    if let Some(general_id) = db::enclave::get_general_id(&chat).await.unwrap() {
        for uid in [&id_a, &id_b] {
            db::enclave::add_member(
                &chat,
                general_id,
                uid,
                lets_chat::models::enclave::EnclaveRole::Member,
            )
            .await
            .unwrap();
        }
    }

    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        email_client: None,
    };
    let app = routes::build_router(state);
    (app, sess_a, id_a, sess_b, id_b)
}

#[tokio::test]
async fn other_user_cannot_fetch_orphan_upload() {
    let (app, sess_a, _id_a, sess_b, _id_b) = app_with_two_users().await;

    // Alice uploads. Orphan: message_id IS NULL.
    let (status, body) = post_upload(app.clone(), &sess_a, "secret.png", TINY_PNG).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = v["file_id"].as_i64().unwrap();

    // Bob tries to fetch Alice's orphan upload -> 403.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/files/{id}"))
        .header(header::COOKIE, format!("session={sess_b}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Alice can still fetch her own orphan.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/files/{id}"))
        .header(header::COOKIE, format!("session={sess_a}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn send_message_with_attachment_renders_inline_image() {
    let (app, sess_a, _id_a, _sess_b, _id_b) = app_with_two_users().await;

    let (status, body) = post_upload(app.clone(), &sess_a, "pic.png", TINY_PNG).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let file_id = v["file_id"].as_i64().unwrap();

    // The seeded "general" room is room id 1.
    let form_body = format!("body=&file_id={file_id}");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/messages")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess_a}"))
        .body(Body::from(form_body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "send message returned non-200"
    );

    // Re-render the room and assert the image partial is in the HTML.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/room/1")
        .header(header::COOKIE, format!("session={sess_a}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains(&format!("/api/files/{file_id}")),
        "rendered HTML should include the attachment URL"
    );
    assert!(
        text.contains("<img src="),
        "rendered HTML should include an <img> tag for the image"
    );
}
