//! LC-96 integration: branding (logo + colors + login text).
//!
//! Covers the DB layer (resolve fallback, color validation,
//! round-trip upsert), the public logo route, the admin form
//! gating, and the CSS-var injection middleware.

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
        let p = std::env::temp_dir().join(format!("lc-branding-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

#[allow(dead_code)]
struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    chat: SqlitePool,
    auth: SqlitePool,
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
        stt_client: None,
        llm_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        chat,
        auth,
    }
}

async fn body_string(res: axum::http::Response<Body>) -> (StatusCode, String) {
    let s = res.status();
    let b = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (s, String::from_utf8_lossy(&b).into_owned())
}

// ── DB layer ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn resolve_falls_back_global_when_enclave_has_no_row() {
    let t = app().await;
    // No enclave row yet; resolution returns the seeded global row.
    let b = db::branding::resolve(&t.chat, db::branding::Scope::Enclave(1))
        .await
        .unwrap();
    assert_eq!(b.scope_kind, "global");
    assert_eq!(b.primary_color, db::branding::DEFAULT_PRIMARY);
}

#[tokio::test]
async fn upsert_round_trip_per_enclave_takes_precedence() {
    let t = app().await;
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Enclave(1),
        None,
        None,
        "#aabbcc",
        "#112233",
        "Welcome",
        "**Hello**",
        "admin",
    )
    .await
    .unwrap();
    let b = db::branding::resolve(&t.chat, db::branding::Scope::Enclave(1))
        .await
        .unwrap();
    assert_eq!(b.primary_color, "#aabbcc");
    assert_eq!(b.login_heading, "Welcome");
}

#[test]
fn enclave_id_from_path_parses_active_enclave() {
    use db::branding::enclave_id_from_path;
    assert_eq!(enclave_id_from_path("/enclave/42"), Some(42));
    assert_eq!(enclave_id_from_path("/enclave/42/settings"), Some(42));
    assert_eq!(enclave_id_from_path("/home"), None);
    assert_eq!(enclave_id_from_path("/enclave/abc/x"), None);
}

// ── HTTP routes ──────────────────────────────────────────────────────────

#[tokio::test]
async fn logo_route_404s_when_no_logo_set() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/branding/logo")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn middleware_injects_brand_vars_into_html_response() {
    let t = app().await;
    // The login page renders text/html and has a <head> tag.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("--brand-primary:"),
        "middleware should stamp the CSS vars; body: {}",
        &body[..body.len().min(300)]
    );
    assert!(
        body.contains("data-lc-brand"),
        "stamped <style> should have the data-lc-brand marker"
    );
}

#[tokio::test]
async fn middleware_skips_injection_for_htmx_requests() {
    let t = app().await;
    // An HX-Request is a fragment fetch with no <head>; the
    // middleware should bail before buffering or injecting.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/login")
        .header("hx-request", "true")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("data-lc-brand"),
        "HX-Request response must not get the injected style block"
    );
}

#[tokio::test]
async fn middleware_picks_enclave_scope_from_path() {
    let t = app().await;
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Enclave(1),
        None,
        None,
        "#ff00ff",
        "#112233",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();
    // Hitting an enclave URL should pick up the enclave's color.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/enclave/1")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert!(
        status.is_success() || status == StatusCode::SEE_OTHER,
        "status {status:?} body {body}"
    );
    if status.is_success() {
        assert!(
            body.contains("--brand-primary:#ff00ff"),
            "enclave-scope branding should win; body head: {}",
            &body[..body.len().min(400)]
        );
    }
    let _ = &t.auth;
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn non_admin_cannot_open_branding_admin_page() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/admin/branding")
        .header(header::COOKIE, format!("session={}", t.member_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_branding_form_persists_text_and_colors() {
    let t = app().await;
    let boundary = "----lc-brand-boundary";
    let body = build_text_multipart(
        boundary,
        &[
            ("primary_color", "#abcdef"),
            ("accent_color", "#001122"),
            ("login_heading", "Hello"),
            ("login_body", "**bold** _italic_"),
        ],
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/branding")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(body))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let stored = db::branding::resolve(&t.chat, db::branding::Scope::Global)
        .await
        .unwrap();
    assert_eq!(stored.primary_color, "#abcdef");
    assert_eq!(stored.accent_color, "#001122");
    assert_eq!(stored.login_heading, "Hello");
    assert_eq!(stored.login_body, "**bold** _italic_");
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_branding_rejects_invalid_color() {
    let t = app().await;
    let boundary = "----lc-brand-bad";
    let body = build_text_multipart(
        boundary,
        &[
            ("primary_color", "not-a-color"),
            ("accent_color", "#112233"),
        ],
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/branding")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(body))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert!(
        status.is_success(),
        "should re-render page with error, not 4xx"
    );
    assert!(
        body.contains("Colors must be"),
        "should surface the validation error"
    );
    // The bad value did not land.
    let stored = db::branding::resolve(&t.chat, db::branding::Scope::Global)
        .await
        .unwrap();
    assert_eq!(stored.primary_color, db::branding::DEFAULT_PRIMARY);
}

// LC-355: login_body is rendered through markdown on the public /login page, so
// it must be length-capped on the write path (LC-153). An over-long body
// re-renders with an inline error and does not persist.
#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_branding_rejects_over_long_login_body() {
    let t = app().await;
    let boundary = "----lc-brand-toolong";
    let long = "a".repeat(2001); // MAX_LOGIN_BODY_CHARS = 2000
    let body = build_text_multipart(
        boundary,
        &[
            ("primary_color", "#abcdef"),
            ("accent_color", "#001122"),
            ("login_body", &long),
        ],
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/branding")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(body))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert!(
        status.is_success(),
        "should re-render with an inline error, got {status}"
    );
    assert!(body.contains("too long"), "should surface the length error");
    let stored = db::branding::resolve(&t.chat, db::branding::Scope::Global)
        .await
        .unwrap();
    assert!(
        stored.login_body.is_empty(),
        "an over-long body must not persist"
    );
}

#[cfg(feature = "standalone")]
fn build_text_multipart(boundary: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

// ── LC-141: branding logo in the authenticated switcher ──────────────────

async fn get_with_session(app: &Router, session: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    body_string(app.clone().oneshot(req).await.unwrap()).await
}

#[tokio::test]
async fn switcher_home_entry_shows_global_logo_when_set() {
    let t = app().await;
    // No logo yet: the Home icon renders the "H" initial, not an <img>.
    let (status, body) = get_with_session(&t.app, &t.admin_session, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("src=\"/branding/logo?"),
        "no logo set means no logo <img> in the switcher",
    );

    // Set a global logo (the upload id need not point at a real file; the
    // switcher only renders the URL).
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Global,
        Some(42),
        None,
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();

    let (status, body) = get_with_session(&t.app, &t.admin_session, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("src=\"/branding/logo?v="),
        "Home switcher icon should render the global logo once set",
    );
}

#[tokio::test]
async fn switcher_enclave_entry_shows_enclave_logo_when_set() {
    let t = app().await;
    // Admin owns (and is a member of) this enclave, so it appears in the
    // switcher when viewing it.
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.admin_id)
        .await
        .unwrap();
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Enclave(eid),
        Some(7),
        None,
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();

    let (status, body) =
        get_with_session(&t.app, &t.admin_session, &format!("/enclave/{eid}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("src=\"/enclave/{eid}/branding/logo?v=")),
        "active enclave's switcher icon should render its logo",
    );
}

// ── LC-142: per-scope favicon ────────────────────────────────────────────

#[tokio::test]
async fn favicon_route_falls_back_to_static_svg_when_unset() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/branding/favicon")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (status, body) = body_string(res).await;
    assert_eq!(status, StatusCode::OK, "favicon route always resolves");
    assert_eq!(ct, "image/svg+xml", "fallback is the bundled SVG");
    assert!(body.contains("<svg"), "served the static favicon markup");
}

#[tokio::test]
async fn upsert_round_trips_favicon_upload_id() {
    let t = app().await;
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Global,
        None,
        Some(77),
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();
    let b = db::branding::resolve(&t.chat, db::branding::Scope::Global)
        .await
        .unwrap();
    assert_eq!(b.favicon_upload_id, Some(77));
}

#[tokio::test]
async fn base_html_links_dynamic_favicon() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let (status, body) = body_string(t.app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("href=\"/branding/favicon?v="),
        "head should link the dynamic favicon route"
    );
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_branding_page_shows_favicon_preview_when_set() {
    let t = app().await;
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Global,
        None,
        Some(99),
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/admin/branding")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let (status, body) = body_string(t.app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Current favicon"), "preview hint rendered");
}

#[tokio::test]
async fn custom_svg_favicon_served_sandboxed() {
    let t = app().await;
    // Stage a real upload row + file, then point the global favicon at it.
    let storage = "lc-test-favicon.svg";
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    let path = lets_chat::db::uploads_dir().join(storage);
    tokio::fs::write(&path, svg).await.unwrap();
    let upload_id = db::uploads::insert_upload(
        &t.chat,
        &t.admin_id,
        "fav.svg",
        "image/svg+xml",
        svg.len() as i64,
        storage,
        None,
    )
    .await
    .unwrap();
    db::branding::upsert(
        &t.chat,
        db::branding::Scope::Global,
        None,
        Some(upload_id),
        "#2563eb",
        "#1d4ed8",
        "",
        "",
        "admin",
    )
    .await
    .unwrap();

    let req = Request::builder()
        .method(Method::GET)
        .uri("/branding/favicon")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let csp = res
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (status, body) = body_string(res).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "image/svg+xml", "served the uploaded SVG");
    assert!(
        csp.contains("sandbox"),
        "uploaded SVG must be sandboxed to neutralize embedded scripts; got {csp:?}"
    );
    assert!(body.contains("<svg"), "served the SVG bytes");
}

// LC-361: every page (via base.html) must emit the JS i18n catalog + helper so
// client-side scripts can localize. A malformed catalog would break all page
// JS, so assert it renders with a known English value.
#[tokio::test]
async fn page_emits_js_i18n_catalog() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/settings")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, body) = body_string(res).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "head: {}",
        &body[..body.len().min(200)]
    );
    assert!(
        body.contains("window.__lcI18n = {"),
        "catalog object missing"
    );
    assert!(body.contains("window.__lcS ="), "catalog helper missing");
    assert!(
        body.contains("callMute: \"Mute\""),
        "expected the English catalog value to render"
    );
}
