//! LC-92 integration: admin maintenance-mode toggle.
//!
//! Covers the standalone-only `POST /admin/maintenance` form, the global
//! middleware that 503s non-admins while the flag is on, the admin
//! carve-out that lets the operator flip the toggle back off from the
//! admin UI, and the moderation-log audit row.
#![cfg(feature = "standalone")]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-maintenance-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    chat: SqlitePool,
    settings: SqlitePool,
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
    let settings_for_test = settings.clone();
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
    let _ = member_id;
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        chat: chat_for_test,
        settings: settings_for_test,
    }
}

async fn send(
    app: &Router,
    sess: Option<&str>,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn non_admin_cannot_post_maintenance_toggle() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/admin/maintenance",
        "enabled=1&message=Brb",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_enables_then_disables_writes_settings_and_audit_log() {
    let t = app().await;

    // Enable.
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        "/admin/maintenance",
        "enabled=1&message=Back+at+17%3A00+UTC",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let mode = db::settings::get_setting(&t.settings, "maintenance_mode")
        .await
        .unwrap();
    assert_eq!(mode.as_deref(), Some("true"));
    let msg = db::settings::get_setting(&t.settings, "maintenance_message")
        .await
        .unwrap();
    assert_eq!(msg.as_deref(), Some("Back at 17:00 UTC"));

    // Disable - the form omits the checkbox and the message field.
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        "/admin/maintenance",
        "message=",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let mode = db::settings::get_setting(&t.settings, "maintenance_mode")
        .await
        .unwrap();
    assert_eq!(mode.as_deref(), Some("false"));

    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let on = actions
        .iter()
        .find(|a| a.action == "maintenance_on")
        .expect("maintenance_on logged");
    let off = actions
        .iter()
        .find(|a| a.action == "maintenance_off")
        .expect("maintenance_off logged");
    assert_eq!(on.actor_user, t.admin_id);
    assert_eq!(on.metadata.as_deref(), Some("Back at 17:00 UTC"));
    assert_eq!(off.actor_user, t.admin_id);
}

#[tokio::test]
async fn non_admin_gets_503_while_maintenance_is_on() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "maintenance_mode", "true")
        .await
        .unwrap();
    db::settings::set_setting(
        &t.settings,
        "maintenance_message",
        "Upgrading the database.",
    )
    .await
    .unwrap();

    let (status, body) = send(&t.app, Some(&t.member_session), Method::GET, "/", "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Upgrading the database."),
        "503 page should embed the maintenance message; got: {body}"
    );
}

#[tokio::test]
async fn admin_bypasses_maintenance_503() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "maintenance_mode", "true")
        .await
        .unwrap();

    // Admin still reaches the settings page so they can flip the toggle
    // back off. AdminUser is satisfied because the request carries the
    // admin's cookie; the maintenance middleware sees role=admin and
    // passes through.
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::GET,
        "/admin/settings",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn login_route_stays_reachable_during_maintenance() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "maintenance_mode", "true")
        .await
        .unwrap();
    // Unauthenticated visitor (no cookie) must still reach /login so they
    // can authenticate as the admin and recover.
    let (status, _) = send(&t.app, None, Method::GET, "/login", "").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn login_surface_stays_reachable_during_maintenance() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "maintenance_mode", "true")
        .await
        .unwrap();
    // LC-22 cutover: a locked-out admin must still be able to reach the
    // sign-in shell during a maintenance window. The maintenance middleware
    // exempts `/login` for that reason. /auth/bunyip/* is also exempt at
    // the middleware layer but we don't hit it here because the test
    // AppState has `bunyip_sso = None` and the handler would panic.
    let (status, _) = send(&t.app, None, Method::GET, "/login", "").await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn ws_upgrade_rejected_for_non_admin_during_maintenance() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "maintenance_mode", "true")
        .await
        .unwrap();
    // The middleware runs before the upgrade handler, so this comes back
    // as a 503 rather than a 101 switching-protocols.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/ws")
        .header(header::COOKIE, format!("session={}", t.member_session))
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}
