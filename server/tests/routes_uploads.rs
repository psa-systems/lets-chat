use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

/// 1x1 transparent PNG, generated once via the `image` crate so the bytes are
/// guaranteed to round-trip through the Phase-23 pipeline (decode → re-encode
/// → strip). The pre-Phase-23 hand-typed PNG fixture is rejected by
/// `image::ImageReader::decode`; using a freshly encoded one keeps the
/// existing tests covering happy paths instead of the decode-rejection branch.
fn tiny_png() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES
        .get_or_init(|| {
            use image::ImageEncoder;
            let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
            let mut buf = Vec::new();
            image::codecs::png::PngEncoder::new(&mut buf)
                .write_image(&img, 1, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
            buf
        })
        .as_slice()
}

/// ZIP fixture bytes: empty ZIP archive (PK\x03\x04 header). Used to assert
/// that renaming a non-image to .png does not bypass magic-byte sniffing.
const TINY_ZIP: &[u8] = b"PK\x03\x04\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

/// A 500x500 PNG, big enough that the 360px preview is meaningfully smaller
/// than the stored original. The exact colours do not matter; the pipeline
/// just needs something it can decode and re-encode.
fn medium_png() -> Vec<u8> {
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(500, 500, image::Rgba([100, 200, 50, 255]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, 500, 500, image::ExtendedColorType::Rgba8)
        .unwrap();
    buf
}

/// Reproduce the upload handler's content-addressed name derivation in the
/// test so we can locate `{sha}.{ext}` and `{sha}_preview.{ext}` on disk
/// without poking at internals. Mirrors `routes::uploads::post_upload`: sha256
/// of the STRIPPED original bytes.
fn expected_preview_path(input: &[u8], mime: &str) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let stem = std::env::temp_dir().join(format!(
        "lets-chat-derive-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::write(&stem, input).unwrap();
    let processed = lets_chat::uploads::pipeline::process_image(&stem, mime).unwrap();
    let _ = std::fs::remove_file(&stem);
    let mut h = Sha256::new();
    h.update(&processed.original_bytes);
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => unreachable!(),
    };
    let preview_name = lets_chat::uploads::preview_storage_name(&format!("{hex}.{ext}"));
    db::uploads_dir().join(preview_name)
}

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
    common::pool(name).await
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
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
    let (status, body) = post_upload(app, &sess, "tiny.png", tiny_png()).await;
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
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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

    let (ctype, body) = multipart_body("file", "tiny.png", tiny_png());
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
    let (status, body) = post_upload(app.clone(), &sess, "tiny.png", tiny_png()).await;
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
    // Phase 23 re-encodes on upload (the strip), so the served bytes are not
    // byte-identical to the upload. Assert semantic round-trip: the response
    // is a valid PNG of the original dimensions.
    let decoded = image::load_from_memory(&bytes).expect("served bytes decode as image");
    assert_eq!(decoded.width(), 1);
    assert_eq!(decoded.height(), 1);
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

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
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
    (app, sess_a, id_a, sess_b, id_b)
}

#[tokio::test]
async fn other_user_cannot_fetch_orphan_upload() {
    let (app, sess_a, _id_a, sess_b, _id_b) = app_with_two_users().await;

    // Alice uploads. Orphan: message_id IS NULL.
    let (status, body) = post_upload(app.clone(), &sess_a, "secret.png", tiny_png()).await;
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
async fn preview_query_returns_smaller_bytes_than_original() {
    let (app, sess, _uid) = app_with_user("eve").await;
    let png = medium_png();
    let (status, body) = post_upload(app.clone(), &sess, "mid.png", &png).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = v["file_id"].as_i64().unwrap();

    async fn fetch(app: Router, sess: &str, url: String) -> (String, Vec<u8>) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(url)
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
        let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap().to_vec();
        (ctype, bytes)
    }
    let (orig_mime, orig_bytes) = fetch(app.clone(), &sess, format!("/api/files/{id}")).await;
    let (prev_mime, prev_bytes) = fetch(app, &sess, format!("/api/files/{id}?size=preview")).await;
    assert_eq!(orig_mime, "image/png");
    assert_eq!(prev_mime, "image/png");
    assert!(
        prev_bytes.len() < orig_bytes.len(),
        "preview ({} B) should be smaller than original ({} B)",
        prev_bytes.len(),
        orig_bytes.len(),
    );

    let preview = image::load_from_memory(&prev_bytes).expect("decode preview");
    assert_eq!(preview.width().max(preview.height()), 360);
}

#[tokio::test]
async fn dedup_upload_heals_missing_preview_on_disk() {
    let (app, sess_a, _id_a, sess_b, _id_b) = app_with_two_users().await;
    let png = medium_png();

    // First upload: writes original + preview. Verify preview is on disk.
    let (status, _body) = post_upload(app.clone(), &sess_a, "pic.png", &png).await;
    assert_eq!(status, StatusCode::OK);
    let preview_path = expected_preview_path(&png, "image/png");
    assert!(
        preview_path.exists(),
        "first upload should have produced preview at {}",
        preview_path.display(),
    );

    // Simulate the Task 3 disk-full-mid-write recovery scenario: original
    // committed, preview missing.
    std::fs::remove_file(&preview_path).unwrap();
    assert!(!preview_path.exists());

    // Second upload of identical bytes by a different user: dedup hit. The
    // handler must heal the missing preview rather than silently leaving the
    // dedup'd row without one.
    let (status, _body) = post_upload(app, &sess_b, "pic.png", &png).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        preview_path.exists(),
        "dedup-hit upload should have healed the missing preview",
    );
}

#[tokio::test]
async fn send_message_with_attachment_renders_inline_image() {
    let (app, sess_a, _id_a, _sess_b, _id_b) = app_with_two_users().await;

    let (status, body) = post_upload(app.clone(), &sess_a, "pic.png", tiny_png()).await;
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
        StatusCode::NO_CONTENT,
        "send message returned non-204 (LC-228)"
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

// ── Voice messages ───────────────────────────────────────────────────────────

/// Minimal Ogg container: `infer` recognises a file as `audio/ogg` from the
/// 4-byte "OggS" magic alone, so this is enough to exercise the voice-upload
/// path without embedding a real Opus stream.
const TINY_OGG: &[u8] = b"OggS\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

/// Build a multipart body carrying `kind=voice`, a `waveform` JSON field, and
/// the `file` field - in the order the upload handler expects.
fn multipart_voice_body(waveform: &str, filename: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----lc-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in [("kind", "voice"), ("waveform", waveform)] {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn post_voice(
    app: Router,
    session: &str,
    waveform: &str,
    filename: &str,
    bytes: &[u8],
) -> (StatusCode, String) {
    let (ctype, body) = multipart_voice_body(waveform, filename, bytes);
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn voice_upload_accepts_audio_and_serves_canonical_mime() {
    let (app, sess, _uid) = app_with_user("voice-alice").await;
    let (status, body) = post_voice(
        app.clone(),
        &sess,
        r#"{"d":2.0,"p":[0.1,0.9,0.5]}"#,
        "clip.ogg",
        TINY_OGG,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
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
    // The sniffed container is canonicalized to an audio/* MIME on store.
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "audio/ogg"
    );
}

#[tokio::test]
async fn voice_upload_rejects_non_media_file() {
    let (app, sess, _uid) = app_with_user("voice-bob").await;
    // A ZIP flagged as kind=voice is not a recognised audio container.
    let (status, _body) = post_voice(app, &sess, "[0.5]", "decoy.ogg", TINY_ZIP).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn voice_upload_tolerates_missing_waveform() {
    let (app, sess, _uid) = app_with_user("voice-carol").await;
    // An empty/garbage waveform must not fail the upload - it just stores NULL.
    let (status, body) = post_voice(app, &sess, "", "clip.ogg", TINY_OGG).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}
