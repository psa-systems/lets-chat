//! LC-698: the deliberate relink path. SSO refuses to re-point an already-linked
//! row at a new subject, so a rotated `sub` (the LC-618 scenario) is recovered by
//! a site admin unlinking the row: the action is audited, the user's sessions are
//! dropped, and the next login links the new subject onto the same row.
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
        let p = std::env::temp_dir().join(format!("lc-unlink-sso-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    auth: SqlitePool,
    chat: SqlitePool,
    admin: String,
    admin_session: String,
    bob: String,
    bob_session: String,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    // Bob is an ordinary SSO user linked to the subject the OP later rotates.
    let bob = db::auth::create_user_from_bunyip(
        &auth,
        "bob",
        "sub-old",
        Some("Bob"),
        Some("bob@example.com"),
    )
    .await
    .unwrap();
    db::auth::mark_email_verified_if_unset(&auth, &bob, "bob@example.com")
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob).await.unwrap();
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
        chat,
        admin,
        admin_session,
        bob,
        bob_session,
    }
}

async fn post(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn admin_unlink_clears_sub_audits_it_and_lets_the_next_login_adopt() {
    let t = app().await;

    let (status, body) = post(
        &t.app,
        &t.admin_session,
        &format!("/admin/users/{}/unlink-sso", t.bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("bob"),
        "the refreshed row is returned: {body}"
    );

    // The subject is cleared, so the old sub resolves to nobody.
    assert!(
        db::auth::get_user_auth_flags_by_bunyip_sub(&t.auth, "sub-old")
            .await
            .unwrap()
            .is_none()
    );

    // The action is in the moderation log, attributed to the acting admin.
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let entry = actions
        .iter()
        .find(|a| a.action == "sso_unlink")
        .expect("sso_unlink is audited");
    assert_eq!(entry.target_user, t.bob);
    assert_eq!(entry.actor_user, t.admin);

    // Bob is signed out everywhere (LC-698: a sub change revokes sessions).
    assert!(
        db::auth::get_user_by_session(&t.auth, &t.bob_session)
            .await
            .unwrap()
            .is_none(),
        "unlinking must revoke the user's sessions",
    );

    // The next login links the rotated subject onto the SAME row.
    assert!(db::auth::link_bunyip_sub(&t.auth, &t.bob, "sub-new")
        .await
        .unwrap());
    assert_eq!(
        db::auth::get_user_auth_flags_by_bunyip_sub(&t.auth, "sub-new")
            .await
            .unwrap(),
        Some((t.bob.clone(), false, false)),
    );
}

#[tokio::test]
async fn unlinking_an_unlinked_user_reports_the_no_op() {
    let t = app().await;
    let uri = format!("/admin/users/{}/unlink-sso", t.bob);

    let (status, _) = post(&t.app, &t.admin_session, &uri).await;
    assert_eq!(status, StatusCode::OK);

    // Second call changes nothing, so it must not report success.
    let (status, body) = post(&t.app, &t.admin_session, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");

    // No second audit entry was written.
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert_eq!(
        actions.iter().filter(|a| a.action == "sso_unlink").count(),
        1,
    );
}

#[tokio::test]
async fn a_non_admin_cannot_unlink() {
    let t = app().await;
    let (status, _) = post(
        &t.app,
        &t.bob_session,
        &format!("/admin/users/{}/unlink-sso", t.admin),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "only site admins may unlink");

    // The admin row keeps its subject.
    let sub: String = sqlx::query_scalar("SELECT bunyip_sub FROM users WHERE id = ?")
        .bind(&t.admin)
        .fetch_one(&t.auth)
        .await
        .unwrap();
    assert!(!sub.is_empty());
}
