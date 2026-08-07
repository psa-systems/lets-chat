//! LC-679: runtime, role-scoped gate for the LLM/AI surface. Covers the three
//! required cases against a representative LLM route (POST compose-assist):
//! flag OFF (403 even for an admin), flag ON + privileged (site admin -> not
//! 403), and flag ON + unprivileged (ordinary member -> 403). Harness mirrors
//! routes_alt_text.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct Setup {
    app: Router,
    settings: sqlx::SqlitePool,
    admin_session: String,
    member_session: String,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member = db::auth::create_user(&auth, "member", "h").await.unwrap();
    // Site admin: privileged for AI everywhere. Also required so the backfill
    // grants General membership (room access) to everyone.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member).await.unwrap();

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
        // LLM configured (the precondition), so the gate turns purely on the
        // runtime flag + the caller's role.
        llm_client: Some(Arc::new(lets_chat::llm::MockLlmClient {
            canned: "Polished draft.".into(),
        })),
        embedding_client: None,
    };
    Setup {
        app: routes::build_router(state),
        settings,
        admin_session,
        member_session,
    }
}

/// POST the writing assistant ("grammar" rewrite) as `session`, returning the
/// status. Room 1 is General; both users are members after the backfill.
async fn post_assist(app: &Router, session: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/compose-assist")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from("action=grammar&body=helo+world"))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn set_flag(settings: &sqlx::SqlitePool, on: bool) {
    db::settings::set_setting(settings, "llm_enabled", if on { "true" } else { "false" })
        .await
        .unwrap();
}

#[tokio::test]
async fn flag_off_forbids_even_a_site_admin() {
    let s = setup().await;
    // Default (no row) is OFF; assert explicitly too.
    set_flag(&s.settings, false).await;
    assert_eq!(
        post_assist(&s.app, &s.admin_session).await,
        StatusCode::FORBIDDEN,
        "flag off must 403 the whole surface, even for an admin"
    );
}

#[tokio::test]
async fn flag_on_allows_a_privileged_role() {
    let s = setup().await;
    set_flag(&s.settings, true).await;
    // Site admin is privileged -> the gate passes and the assist renders (200).
    assert_eq!(post_assist(&s.app, &s.admin_session).await, StatusCode::OK);
}

#[tokio::test]
async fn flag_on_forbids_an_unprivileged_user() {
    let s = setup().await;
    set_flag(&s.settings, true).await;
    // Ordinary member of the room: not a site admin, not a room moderator, not
    // an enclave Owner/Admin -> 403 despite the flag being on.
    assert_eq!(
        post_assist(&s.app, &s.member_session).await,
        StatusCode::FORBIDDEN,
        "an unprivileged member must be refused on the server, not just in the UI"
    );
}

#[tokio::test]
async fn flag_defaults_off_when_unset() {
    let s = setup().await;
    // No set_flag call: the settings row is absent, which must read as OFF.
    assert_eq!(
        post_assist(&s.app, &s.admin_session).await,
        StatusCode::FORBIDDEN,
        "absent llm_enabled row must default the surface OFF in production"
    );
}
