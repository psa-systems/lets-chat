//! LC-334: message-report HTTP flow. File-scoped to standalone because the
//! review queue (/admin/reports) lives in the `#[cfg(standalone)]` admin module.
#![cfg(feature = "standalone")]

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
        let p = std::env::temp_dir().join(format!("lc-reports-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    chat: SqlitePool,
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        chat,
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn report_flow_file_view_and_resolve() {
    let t = app().await;
    // Admin posts a message; member reports it.
    let mid = db::chat::insert_message(&t.chat, 1, &t.admin_id, "buy cheap pills now")
        .await
        .unwrap();

    let (status, body) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        &format!("/messages/{mid}/report"),
        "category=spam&note=clearly+spam",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "report submit: {body}");

    // The open queue shows the report (excerpt, reporter, category).
    let (status, queue) = send(
        &t.app,
        Some(&t.admin_session),
        Method::GET,
        "/admin/reports",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(queue.contains("buy cheap pills now"), "excerpt missing");
    assert!(queue.contains("member"), "reporter label missing");
    assert!(queue.contains("clearly spam"), "note missing");

    let open = db::reports::list_open(&t.chat).await.unwrap();
    assert_eq!(open.len(), 1);
    let report_id = open[0].id;

    // Resolve it; the OOB response re-renders the (now empty) list region.
    let (status, oob) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        &format!("/admin/reports/{report_id}/resolve"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        oob.contains("admin-reports-list"),
        "OOB list region missing"
    );
    assert_eq!(db::reports::count_open(&t.chat).await.unwrap(), 0);
}

#[tokio::test]
async fn duplicate_report_is_a_no_op() {
    let t = app().await;
    let mid = db::chat::insert_message(&t.chat, 1, &t.admin_id, "spammy text")
        .await
        .unwrap();
    for _ in 0..2 {
        let (status, _) = send(
            &t.app,
            Some(&t.member_session),
            Method::POST,
            &format!("/messages/{mid}/report"),
            "category=spam",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(db::reports::count_open(&t.chat).await.unwrap(), 1);
}

#[tokio::test]
async fn invalid_category_rejected() {
    let t = app().await;
    let mid = db::chat::insert_message(&t.chat, 1, &t.admin_id, "hello")
        .await
        .unwrap();
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        &format!("/messages/{mid}/report"),
        "category=bogus",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn member_cannot_view_admin_queue() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::GET,
        "/admin/reports",
        "",
    )
    .await;
    assert!(!status.is_success(), "member must not see the admin queue");
}
