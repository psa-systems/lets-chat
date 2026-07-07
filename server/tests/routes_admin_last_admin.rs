//! LC-352: the admin user-management actions must not strip admin capability
//! from the last active admin (role=admin, not banned), which would lock
//! everyone out of /admin. Covers demote (role change away from admin) and ban;
//! delete is guarded the same way but is unreachable for the sole admin (the
//! self-delete check already blocks it).
//!
//! Admin routes are `#[cfg(feature = "standalone")]`, so this whole file is too.
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
        let p = std::env::temp_dir().join(format!("lc-admin-floor-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    auth: SqlitePool,
    alice: String,
    alice_session: String,
    bob: String,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    // Alice is the sole admin.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
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
        secret_key: None,
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
        auth,
        alice,
        alice_session,
        bob,
    }
}

async fn post(app: &Router, sess: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn role_of(auth: &SqlitePool, id: &str) -> String {
    sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(id)
        .fetch_one(auth)
        .await
        .unwrap()
}

#[tokio::test]
async fn demoting_the_last_admin_is_rejected() {
    let t = app().await;
    let (status, body) = post(
        &t.app,
        &t.alice_session,
        &format!("/admin/users/{}/role", t.alice),
        "role=user",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        role_of(&t.auth, &t.alice).await,
        "admin",
        "alice stays admin"
    );
}

#[tokio::test]
async fn demote_allowed_once_a_second_admin_exists() {
    let t = app().await;
    // Promote bob -> now two active admins.
    let (s, _) = post(
        &t.app,
        &t.alice_session,
        &format!("/admin/users/{}/role", t.bob),
        "role=admin",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Demoting alice is now fine: bob remains.
    let (s, body) = post(
        &t.app,
        &t.alice_session,
        &format!("/admin/users/{}/role", t.alice),
        "role=user",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(role_of(&t.auth, &t.alice).await, "user");
}

#[tokio::test]
async fn banning_the_last_admin_is_rejected() {
    let t = app().await;
    let (status, body) = post(
        &t.app,
        &t.alice_session,
        &format!("/admin/users/{}/ban", t.alice),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let banned: bool = sqlx::query_scalar("SELECT is_banned FROM users WHERE id = ?")
        .bind(&t.alice)
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert!(!banned, "the last admin must not be banned");
}
