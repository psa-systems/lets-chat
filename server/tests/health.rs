//! LC-581: production smoke / health check.
//!
//! The incident was a gateway timeout: production boot never reached
//! `axum::serve`, so Traefik had no upstream and returned 504. These tests pin
//! the two probes that make that state observable and recoverable instead of a
//! black box:
//!
//! - `/healthz` (liveness) is dependency-free and is what the container
//!   HEALTHCHECK targets, so a listening process reports healthy at once and is
//!   never culled for a slow dependency.
//! - `/readyz` (readiness) pings every backing store and 503s when one is down,
//!   so a load balancer / monitor can drain or alert without restarting a fine
//!   process.
//!
//! Both must also answer during a maintenance window, or the orchestrator would
//! read maintenance mode as an outage.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-health-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

struct Setup {
    app: Router,
    settings: sqlx::SqlitePool,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings: settings.clone(),
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
    Setup {
        app: routes::build_router(state.clone()),
        settings,
    }
}

/// Liveness is dependency-free and always 200 for a listening process. This is
/// the exact signal the container HEALTHCHECK reads.
#[tokio::test]
async fn healthz_is_ok_and_dependency_free() {
    let s = setup().await;
    let (status, body) = get(&s.app, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"status\":\"ok\""), "got: {body}");
}

/// With every backing store reachable (the test harness state), readiness is
/// 200 and each store is reported healthy.
#[tokio::test]
async fn readyz_is_ready_when_stores_are_up() {
    let s = setup().await;
    let (status, body) = get(&s.app, "/readyz").await;
    assert_eq!(status, StatusCode::OK, "got body: {body}");
    assert!(body.contains("\"status\":\"ready\""), "got: {body}");
    assert!(body.contains("\"auth_db\":true"), "got: {body}");
    assert!(body.contains("\"chat_db\":true"), "got: {body}");
    assert!(body.contains("\"settings_db\":true"), "got: {body}");
    // No SSO client is wired in this harness, so readiness reports it absent
    // without flipping the 503 - a runtime OP outage degrades login only.
    assert!(body.contains("\"sso\":false"), "got: {body}");
}

/// Both probes bypass the maintenance gate. If they did not, enabling
/// maintenance mode would make the orchestrator and any LB read the instance as
/// down, which is the opposite of what a maintenance window intends.
#[tokio::test]
async fn probes_answer_during_maintenance_mode() {
    let s = setup().await;
    db::settings::set_setting(&s.settings, "maintenance_mode", "true")
        .await
        .unwrap();

    // A normal page is gated (503 maintenance page) for an anonymous visitor...
    let (gated, _) = get(&s.app, "/").await;
    assert_ne!(
        gated,
        StatusCode::OK,
        "sanity: maintenance mode should gate the home page"
    );

    // ...but the probes still answer.
    let (live, _) = get(&s.app, "/healthz").await;
    assert_eq!(live, StatusCode::OK, "liveness must survive maintenance");
    let (ready, _) = get(&s.app, "/readyz").await;
    assert_eq!(ready, StatusCode::OK, "readiness must survive maintenance");
}
