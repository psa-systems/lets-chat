//! LC-878: the last five native-POST settings forms (personal emoji add/delete,
//! personal canned-response add/delete, room moderator grant) now answer the
//! same dual-mode contract as the rest of the settings page (LC-739):
//! `redirect_or_hx` sends an htmx submit `HX-Redirect` to the exact URL a
//! no-JS submit gets as a plain redirect, and a failed submit answers with a
//! non-2xx status instead of a 2xx. Modelled on
//! `routes_enclave_settings_feedback.rs`.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!(
                "lc-personal-list-forms-tests-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

struct TestApp {
    app: Router,
    owner_session: String,
    owner_id: String,
    chat: SqlitePool,
}

/// A site admin on the seeded General enclave (id 1, room id 1), who performs
/// every save in this suite.
async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let owner_id = db::auth::create_user(&auth, "owner", "hash").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&owner_id)
        .execute(&auth)
        .await
        .unwrap();
    let owner_session = db::auth::create_session(&auth, &owner_id).await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        owner_session,
        owner_id,
        chat,
    }
}

/// POST a url-encoded form body; `hx` decides whether the request carries
/// `HX-Request`.
async fn post_form(
    app: &Router,
    sess: &str,
    uri: &str,
    body: &str,
    hx: bool,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if hx {
        req = req.header("hx-request", "true");
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body =
        String::from_utf8_lossy(&to_bytes(res.into_body(), 1 << 20).await.unwrap()).into_owned();
    (status, headers, body)
}

/// 1x1 PNG, valid enough for `infer` to sniff as `image/png`.
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

/// Build a `shortcode` + `file` multipart body, matching the field set
/// `ingest_emoji_multipart` expects.
fn emoji_multipart_body(shortcode: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----lc-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"shortcode\"\r\n\r\n");
    body.extend_from_slice(shortcode.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"e.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn post_multipart(
    app: &Router,
    sess: &str,
    uri: &str,
    ctype: &str,
    body: Vec<u8>,
    hx: bool,
) -> (StatusCode, axum::http::HeaderMap) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, ctype);
    if hx {
        req = req.header("hx-request", "true");
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    (res.status(), res.headers().clone())
}

// ── Personal emoji upload (POST /settings/emojis) ──────────────────────────

#[tokio::test]
async fn emoji_upload_htmx_success_returns_hx_redirect() {
    let t = app().await;
    let (ctype, body) = emoji_multipart_body("partyparrot", tiny_png());
    let (status, headers) = post_multipart(
        &t.app,
        &t.owner_session,
        "/settings/emojis",
        &ctype,
        body,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").unwrap(),
        "/settings?ok=emoji-added#emoji"
    );
}

#[tokio::test]
async fn emoji_upload_no_js_success_redirects() {
    let t = app().await;
    let (ctype, body) = emoji_multipart_body("partyparrot", tiny_png());
    let (status, headers) = post_multipart(
        &t.app,
        &t.owner_session,
        "/settings/emojis",
        &ctype,
        body,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/settings?ok=emoji-added#emoji"
    );
}

#[tokio::test]
async fn emoji_upload_duplicate_shortcode_is_conflict_not_2xx() {
    let t = app().await;
    let (ctype, body) = emoji_multipart_body("dup", tiny_png());
    let (status, _) = post_multipart(
        &t.app,
        &t.owner_session,
        "/settings/emojis",
        &ctype,
        body,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (ctype, body) = emoji_multipart_body("dup", tiny_png());
    let (status, _) = post_multipart(
        &t.app,
        &t.owner_session,
        "/settings/emojis",
        &ctype,
        body,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

// ── Personal emoji delete (POST /settings/emojis/{id}/delete) ──────────────

#[tokio::test]
async fn emoji_delete_htmx_success_returns_hx_redirect() {
    let t = app().await;
    let id =
        db::custom_emojis::insert_for_user(&t.chat, &t.owner_id, "todel", "x.png", "image/png", 10)
            .await
            .unwrap();
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        &format!("/settings/emojis/{id}/delete"),
        "",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").unwrap(),
        "/settings?ok=emoji-deleted#emoji"
    );
}

#[tokio::test]
async fn emoji_delete_no_js_success_redirects() {
    let t = app().await;
    let id = db::custom_emojis::insert_for_user(
        &t.chat,
        &t.owner_id,
        "todel2",
        "x.png",
        "image/png",
        10,
    )
    .await
    .unwrap();
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        &format!("/settings/emojis/{id}/delete"),
        "",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/settings?ok=emoji-deleted#emoji"
    );
}

#[tokio::test]
async fn emoji_delete_nonexistent_is_not_found_not_2xx() {
    let t = app().await;
    let (status, _, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/emojis/999999/delete",
        "",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Canned response create (POST /settings/canned) ──────────────────────────

#[tokio::test]
async fn canned_create_htmx_success_returns_hx_redirect() {
    let t = app().await;
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned",
        "name=sig&description=&body=hello",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").unwrap(),
        "/settings?ok=canned-added#canned"
    );
}

#[tokio::test]
async fn canned_create_no_js_success_redirects() {
    let t = app().await;
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned",
        "name=sig2&description=&body=hello",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/settings?ok=canned-added#canned"
    );
}

#[tokio::test]
async fn canned_create_duplicate_name_is_conflict_not_2xx() {
    let t = app().await;
    let (status, _, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned",
        "name=dupe&description=&body=hello",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned",
        "name=dupe&description=&body=again",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn canned_create_invalid_name_is_bad_request_not_2xx() {
    let t = app().await;
    let (status, _, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned",
        "name=has+space&description=&body=hello",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── Canned response delete (POST /settings/canned/{id}/delete) ─────────────

#[tokio::test]
async fn canned_delete_htmx_success_returns_hx_redirect() {
    let t = app().await;
    let id = db::slash::insert_canned(&t.chat, &t.owner_id, "todel", "", "bye")
        .await
        .unwrap();
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        &format!("/settings/canned/{id}/delete"),
        "",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("hx-redirect").unwrap(),
        "/settings?ok=canned-deleted#canned"
    );
}

#[tokio::test]
async fn canned_delete_no_js_success_redirects() {
    let t = app().await;
    let id = db::slash::insert_canned(&t.chat, &t.owner_id, "todel2", "", "bye")
        .await
        .unwrap();
    let (status, headers, _) = post_form(
        &t.app,
        &t.owner_session,
        &format!("/settings/canned/{id}/delete"),
        "",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/settings?ok=canned-deleted#canned"
    );
}

#[tokio::test]
async fn canned_delete_nonexistent_is_not_found_not_2xx() {
    let t = app().await;
    let (status, _, _) = post_form(
        &t.app,
        &t.owner_session,
        "/settings/canned/999999/delete",
        "",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Room moderator grant (POST /room/{id}/moderators) ───────────────────────

#[tokio::test]
async fn room_grant_htmx_success_returns_hx_redirect() {
    let t = app().await;
    let body = format!("user_id={}&role=moderator", t.owner_id);
    // Grant the owner an override on their own room 1; require_can_manage
    // authorizes on org-admin role regardless of target, and the owner is
    // already a General member so the membership check passes too.
    let (status, headers, _) =
        post_form(&t.app, &t.owner_session, "/room/1/moderators", &body, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("hx-redirect").unwrap(), "/room/1/manage");
}

#[tokio::test]
async fn room_grant_no_js_success_redirects() {
    let t = app().await;
    let body = format!("user_id={}&role=moderator", t.owner_id);
    let (status, headers, _) =
        post_form(&t.app, &t.owner_session, "/room/1/moderators", &body, false).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/room/1/manage");
}

#[tokio::test]
async fn room_grant_non_member_target_is_bad_request_not_2xx() {
    let t = app().await;
    let body = "user_id=no-such-user-id&role=moderator";
    let (status, _, _) =
        post_form(&t.app, &t.owner_session, "/room/1/moderators", body, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
