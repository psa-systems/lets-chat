//! LC-740: every user-facing file input renders through
//! `partials/file_picker.html`, so all of them look the same, echo the chosen
//! filename, and carry the attributes `assets/file_picker.js` validates against.
//!
//! Four of these inputs used to be raw `<input type="file">` (both admin
//! branding fields, the restore archive, the personal custom emoji) and a fifth
//! (the enclave icon) was `sr-only` with no echo at all, so picking a file
//! changed nothing on screen. These assertions pin the shape at the rendered
//! HTML, and each `data-lc-max-bytes` is checked against the cap the handler
//! behind that field actually enforces.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-file-picker-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
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
        admin_session,
    }
}

async fn get(app: &Router, session: &str, uri: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "GET {uri}");
    let bytes = to_bytes(res.into_body(), 8 << 20).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The whole `<input ...>` tag for `id` (the partial wraps its attributes
/// across several lines).
fn input_tag<'a>(body: &'a str, id: &str) -> &'a str {
    let start = body
        .find(&format!("<input id=\"{id}\""))
        .unwrap_or_else(|| panic!("no file input rendered with id={id}"));
    let end = body[start..]
        .find('>')
        .unwrap_or_else(|| panic!("unterminated input tag for id={id}"));
    &body[start..start + end]
}

/// Assert the full picker shape for `id`: styled trigger label, filename echo,
/// `sr-only` input carrying the validation attributes, and the error slot.
fn assert_picker(body: &str, id: &str, max_bytes: &str) {
    let input = input_tag(body, id);
    assert!(input.contains("type=\"file\""), "{id}: is a file input");
    assert!(
        input.contains("class=\"sr-only\""),
        "{id}: input is sr-only"
    );
    assert!(
        input.contains("data-lc-file-picker"),
        "{id}: opts into the shared handler"
    );
    assert!(
        input.contains(&format!("data-lc-max-bytes=\"{max_bytes}\"")),
        "{id}: data-lc-max-bytes must be {max_bytes}, got: {input}"
    );
    for attr in ["data-lc-no-file", "data-lc-err-type", "data-lc-err-size"] {
        assert!(
            input.contains(&format!("{attr}=\"")),
            "{id}: {attr} is set and non-empty"
        );
    }
    assert!(
        body.contains(&format!(
            "<label for=\"{id}\" class=\"btn btn-secondary btn-sm\">"
        )),
        "{id}: styled trigger label"
    );
    assert!(
        body.contains(&format!("data-lc-picker-filename=\"{id}\"")),
        "{id}: filename echo"
    );
    assert!(
        body.contains(&format!(
            "<p data-lc-picker-error=\"{id}\" hidden role=\"alert\""
        )),
        "{id}: inline role=alert error slot"
    );
}

#[tokio::test]
async fn settings_page_pickers_share_the_one_shape() {
    let t = app().await;
    let body = get(&t.app, &t.admin_session, "/settings").await;
    // MAX_AVATAR_BYTES in routes/settings.rs.
    assert_picker(&body, "lc-avatar-input", "1048576");
    // MAX_EMOJI_BYTES in routes/custom_emojis.rs.
    assert_picker(&body, "lc-user-emoji-file", "262144");
}

#[tokio::test]
async fn enclave_settings_pickers_share_the_one_shape() {
    let t = app().await;
    let body = get(&t.app, &t.admin_session, "/enclave/1/settings").await;
    assert_picker(&body, "lc-emoji-file", "262144");
    // post_icon parses through persist_brand_file's 1 MiB cap.
    assert_picker(&body, "lc-enclave-icon-input", "1048576");
}

#[tokio::test]
async fn enclave_branding_picker_shares_the_one_shape() {
    let t = app().await;
    let body = get(&t.app, &t.admin_session, "/enclave/1/branding").await;
    assert_picker(&body, "lc-logo-input", "1048576");
}

#[tokio::test]
async fn admin_branding_pickers_share_the_one_shape() {
    let t = app().await;
    let body = get(&t.app, &t.admin_session, "/admin/branding").await;
    // persist_brand_file caps both fields at 1 MiB; only the types differ.
    assert_picker(&body, "logo", "1048576");
    assert_picker(&body, "favicon", "1048576");
}

#[tokio::test]
async fn restore_archive_picker_shares_the_one_shape() {
    let t = app().await;
    let body = get(&t.app, &t.admin_session, "/admin/backup-restore").await;
    // DefaultBodyLimit on POST /admin/restore in routes/admin.rs.
    assert_picker(&body, "lc-restore-archive", "10737418240");
}
