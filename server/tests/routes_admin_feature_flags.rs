#![cfg(feature = "standalone")]
//! LC-1056: the Admin > Feature flags tab. Standalone-only because
//! `routes::admin` is `#[cfg(feature = "standalone")]`.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [29u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-flags-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    app: Router,
    admin_session: String,
    chat: SqlitePool,
    settings: SqlitePool,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
    let settings_for_test = settings.clone();
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
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
    Fixture {
        app: routes::build_router(state),
        admin_session,
        chat: chat_for_test,
        settings: settings_for_test,
    }
}

async fn send(
    app: &Router,
    method: Method,
    session: &str,
    uri: &str,
    form: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={session}"));
    if form.is_some() {
        req = req
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("hx-request", "true");
    }
    let res = app
        .clone()
        .oneshot(
            req.body(Body::from(form.unwrap_or("").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn setting(f: &Fixture, key: &str) -> Option<String> {
    db::settings::get_setting(&f.settings, key).await.unwrap()
}

#[tokio::test]
async fn feature_flags_tab_holds_every_switch_and_settings_no_longer_does() {
    let f = setup().await;
    let (st, html) = send(
        &f.app,
        Method::GET,
        &f.admin_session,
        "/admin/feature-flags",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    for id in [
        "lc-flag-ai",
        "lc-flag-remote-control",
        "lc-flag-transcription",
    ] {
        assert!(html.contains(id), "{id} card renders");
    }
    assert!(html.contains(r#"href="/admin/feature-flags" class="lc-set-tab" aria-current="page""#));
    // Fresh install: both three-state flags start Off.
    assert_eq!(html.matches(r#"value="off" checked"#).count(), 2, "{html}");

    let (_, settings) = send(
        &f.app,
        Method::GET,
        &f.admin_session,
        "/admin/settings",
        None,
    )
    .await;
    assert!(!settings.contains(r#"action="/admin/settings/llm""#));
    assert!(!settings.contains(r#"action="/admin/settings/remote-control""#));
}

#[tokio::test]
async fn saving_a_rollout_persists_both_keys_and_audits() {
    let f = setup().await;
    let (st, _) = send(
        &f.app,
        Method::POST,
        &f.admin_session,
        "/admin/settings/transcription",
        Some("rollout=admins"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        setting(&f, "transcription_enabled").await.as_deref(),
        Some("true")
    );
    assert_eq!(
        setting(&f, "transcription_audience").await.as_deref(),
        Some("admins")
    );

    let (st, _) = send(
        &f.app,
        Method::POST,
        &f.admin_session,
        "/admin/settings/remote-control",
        Some("rollout=everyone"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        setting(&f, "remote_control_enabled").await.as_deref(),
        Some("true")
    );
    assert_eq!(
        setting(&f, "remote_control_audience").await.as_deref(),
        Some("everyone")
    );

    let (st, _) = send(
        &f.app,
        Method::POST,
        &f.admin_session,
        "/admin/settings/transcription",
        Some("rollout=off"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        setting(&f, "transcription_enabled").await.as_deref(),
        Some("false")
    );

    let actions: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, reason FROM mod_actions WHERE action LIKE 'transcription_%' OR action LIKE 'remote_control_%' ORDER BY id",
    )
    .fetch_all(&f.chat)
    .await
    .unwrap();
    let kinds: Vec<&str> = actions.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        kinds,
        ["transcription_on", "remote_control_on", "transcription_off"]
    );
    assert_eq!(actions[0].1.as_deref(), Some("rollout=admins"));

    let (_, html) = send(
        &f.app,
        Method::GET,
        &f.admin_session,
        "/admin/feature-flags",
        None,
    )
    .await;
    assert!(
        html.contains(r#"value="everyone" checked"#),
        "remote control shows Everyone"
    );
}

#[tokio::test]
async fn pre_rollout_remote_control_switch_reads_as_everyone() {
    let f = setup().await;
    // An LC-853 deployment that only ever wrote the on/off row.
    db::settings::set_setting(&f.settings, "remote_control_enabled", "true")
        .await
        .unwrap();
    let (_, html) = send(
        &f.app,
        Method::GET,
        &f.admin_session,
        "/admin/feature-flags",
        None,
    )
    .await;
    assert!(html.contains(r#"value="everyone" checked"#), "{html}");
}
