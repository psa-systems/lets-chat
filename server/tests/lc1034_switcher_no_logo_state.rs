//! LC-1034: the switcher (`load_switcher` in `routes/mod.rs`) must agree with
//! `has_icon` on the enclave settings page (`routes/enclave.rs`) about whether
//! an enclave has its own logo. Both go through `db::branding::resolve`'s
//! row-level contract: an enclave with no branding row falls back to the
//! global logo, but an enclave with a branding row whose `logo_upload_id` is
//! `NULL` (colors customized, no logo uploaded) has no logo at all. LC-782's
//! batched `logo_ids_for_enclaves` used to conflate "no row" with "row with a
//! null logo" via `.flatten()`, silently substituting the global logo for the
//! null-logo case too. These tests render `/inbox` (which includes the
//! switcher partial) and assert on the actual HTML for all three cases.

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
        let p = std::env::temp_dir().join(format!("lc-1034-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    viewer_id: String,
    viewer_session: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let viewer_id = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&viewer_id)
        .execute(&auth)
        .await
        .unwrap();
    let viewer_session = db::auth::create_session(&auth, &viewer_id).await.unwrap();
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
    let app = routes::build_router(state);
    TestApp {
        app,
        viewer_id,
        viewer_session,
        chat,
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
    let body =
        String::from_utf8(to_bytes(resp.into_body(), 10 << 20).await.unwrap().to_vec()).unwrap();
    (status, body)
}

/// The tile for enclave `id`: the `<a href="/enclave/{id}">...</a>` block
/// from the switcher partial, so logo-vs-initial assertions only look inside
/// that one tile and not at some other enclave's tile or the Home tile.
fn tile_for(body: &str, id: i64) -> &str {
    let open = format!("href=\"/enclave/{id}\"");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no switcher tile for enclave {id} found in body: {body}"));
    let end = body[start..]
        .find("</a>")
        .map(|i| start + i)
        .unwrap_or(body.len());
    &body[start..end]
}

async fn stage_global_logo(chat: &SqlitePool, auth_user: &str) -> i64 {
    let storage = format!("lc-1034-global-logo-{}.png", std::process::id());
    let path = lets_chat::db::uploads_dir().join(&storage);
    tokio::fs::write(&path, b"not a real png").await.unwrap();
    let upload_id =
        db::uploads::insert_upload(chat, auth_user, "logo.png", "image/png", 14, &storage, None)
            .await
            .unwrap();
    db::branding::upsert(
        chat,
        db::branding::Scope::Global,
        Some(upload_id),
        None,
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        auth_user,
    )
    .await
    .unwrap();
    upload_id
}

#[tokio::test(flavor = "current_thread")]
async fn enclave_with_null_logo_row_shows_no_logo_even_with_global_set() {
    let t = app().await;
    stage_global_logo(&t.chat, &t.viewer_id).await;

    // General (id 1) customizes colors but never uploads its own logo: a
    // branding row exists with `logo_upload_id = NULL`.
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Enclave(1),
        None,
        None,
        "#aabbcc",
        "#112233",
        "",
        "",
        &t.viewer_id,
    )
    .await
    .unwrap();

    let (status, body) = get(&t.app, &t.viewer_session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);

    let tile = tile_for(&body, 1);
    assert!(
        !tile.contains("<img"),
        "enclave with a null-logo branding row must not inherit the global logo, got tile: {tile}"
    );
    assert!(
        tile.contains("font-semibold"),
        "enclave with no own logo must render its initial glyph, got tile: {tile}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn enclave_with_no_branding_row_falls_back_to_global_logo() {
    let t = app().await;
    stage_global_logo(&t.chat, &t.viewer_id).await;

    // General (id 1) has no branding row at all.
    let (status, body) = get(&t.app, &t.viewer_session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);

    let tile = tile_for(&body, 1);
    assert!(
        tile.contains("<img") && tile.contains("/enclave/1/branding/logo"),
        "enclave with no branding row must fall back to the global logo, got tile: {tile}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn enclave_with_its_own_logo_shows_its_own_logo() {
    let t = app().await;
    stage_global_logo(&t.chat, &t.viewer_id).await;

    let storage = format!("lc-1034-enclave-logo-{}.png", std::process::id());
    let path = lets_chat::db::uploads_dir().join(&storage);
    tokio::fs::write(&path, b"not a real png either")
        .await
        .unwrap();
    let enclave_logo_id = db::uploads::insert_upload(
        &t.chat,
        &t.viewer_id,
        "enclave-logo.png",
        "image/png",
        21,
        &storage,
        None,
    )
    .await
    .unwrap();
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Enclave(1),
        Some(enclave_logo_id),
        None,
        "#aabbcc",
        "#112233",
        "",
        "",
        &t.viewer_id,
    )
    .await
    .unwrap();

    let (status, body) = get(&t.app, &t.viewer_session, "/inbox").await;
    assert_eq!(status, StatusCode::OK);

    let tile = tile_for(&body, 1);
    assert!(
        tile.contains("<img") && tile.contains("/enclave/1/branding/logo"),
        "enclave with its own logo must render it, got tile: {tile}"
    );
}
