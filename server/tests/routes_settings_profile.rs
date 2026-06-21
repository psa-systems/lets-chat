//! LC-347: a profile/avatar save must confirm success. `post_profile` redirects
//! to `/settings?saved=1` (so the existing "Saved." flash fires), and the
//! settings-page avatar preview is cache-busted by the avatar file's mtime
//! rather than the build-constant `asset_version`, so the new image is not
//! masked by the avatar route's `max-age=300`.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

/// Point the process-global data dir at a writable temp dir so avatar writes
/// land somewhere real (the default `/data` is not writable under the test
/// runner). First writer wins, matching the uploads tests.
fn ensure_tempdir() {
    static TEMPDIR: OnceLock<()> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

/// Smallest input `sniff_image_ext` accepts as PNG: the 8-byte signature plus a
/// trailing byte so the written file is non-trivial.
const PNG_FIXTURE: &[u8] = b"\x89PNG\r\n\x1a\n\x00";
const BOUNDARY: &str = "lc347boundary";

fn multipart_avatar_body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(PNG_FIXTURE);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// LC-439: a multipart body whose `avatar` field is `n` bytes - large enough to
/// exceed both the 1 MiB avatar cap and (at n > 2 MiB) the old default ~2 MiB
/// request-body limit, so this exercises the raised per-route limit.
fn multipart_avatar_body_sized(n: usize) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.resize(body.len() + n, 0u8);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn post_avatar(app: &Router, sess: Option<&str>) -> axum::response::Response {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/settings/profile")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
    if let Some(s) = sess {
        req = req.header(header::COOKIE, format!("session={s}"));
    }
    let req = req.body(Body::from(multipart_avatar_body())).unwrap();
    app.clone().oneshot(req).await.unwrap()
}

struct Setup {
    app: Router,
    auth: sqlx::SqlitePool,
    user_id: String,
    session: String,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let user_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    // secret_key is Some below; satisfy the historical 2FA-enrollment harness
    // pattern (now a no-op since the middleware was retired in LC-22, but
    // harmless and consistent with the other settings tests).
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat,
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
    };
    Setup {
        app: routes::build_router(state),
        auth,
        user_id,
        session,
    }
}

#[tokio::test]
async fn avatar_upload_redirects_to_saved_flash() {
    let s = setup().await;
    let resp = post_avatar(&s.app, Some(&s.session)).await;

    // LC-347: the redirect carries ?saved=1 so get_settings flashes "Saved.".
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/settings?saved=1"
    );

    // The avatar was actually persisted.
    let user = db::auth::find_user_by_id(&s.auth, &s.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.avatar_ext.as_deref(), Some("png"));

    let _ = tokio::fs::remove_file(db::avatars_dir().join(format!("{}.png", s.user_id))).await;
}

#[tokio::test]
async fn settings_avatar_preview_cache_busts_not_asset_version() {
    let s = setup().await;
    post_avatar(&s.app, Some(&s.session)).await;

    // Render the settings page and confirm the avatar <img> is NOT keyed by the
    // build-constant asset_version ("test"), which never changes on upload.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/settings")
        .header(header::COOKIE, format!("session={}", s.session))
        .body(Body::empty())
        .unwrap();
    let resp = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);

    let needle = format!("/avatars/{}?v=", s.user_id);
    assert!(html.contains(&needle), "avatar img not rendered");
    assert!(
        !html.contains(&format!("/avatars/{}?v=test", s.user_id)),
        "avatar img still keyed by static asset_version; cache-bust ineffective"
    );

    let _ = tokio::fs::remove_file(db::avatars_dir().join(format!("{}.png", s.user_id))).await;
}

// LC-356: a profile validation failure flashes inline on /settings (a redirect
// with ?error=) instead of throwing a full-page error.
#[tokio::test]
async fn over_long_display_name_redirects_with_inline_error() {
    let s = setup().await;
    let long = "n".repeat(65); // MAX_DISPLAY_NAME_CHARS = 64
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"display_name\"\r\n\r\n");
    body.extend_from_slice(long.as_bytes());
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/profile")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header(header::COOKIE, format!("session={}", s.session))
        .body(Body::from(body))
        .unwrap();
    let resp = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        loc.starts_with("/settings?error="),
        "expected inline error flash, got loc: {loc}"
    );
}

/// LC-439: an oversized avatar from htmx must reach the handler and come back as
/// a visible error fragment (200 + .lc-status--err), NOT a 413 the body-limit
/// layer would emit (which htmx silently ignores -> the original silent bug).
/// The 3 MiB body also exceeds the old default ~2 MiB cap, proving the raised
/// per-route limit.
#[tokio::test]
async fn oversized_avatar_hx_returns_error_fragment_not_413() {
    let s = setup().await;
    let body = multipart_avatar_body_sized(3 * 1024 * 1024);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/profile")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header(header::COOKIE, format!("session={}", s.session))
        .header("HX-Request", "true")
        .body(Body::from(body))
        .unwrap();
    let resp = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "oversized avatar should reach the handler (raised body limit), not 413"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("lc-status--err"), "no error fragment: {html}");
    assert!(html.contains("1 MiB"), "no size reason shown: {html}");
}
