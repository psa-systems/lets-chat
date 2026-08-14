//! LC-705: workspace-wide "Catch me up" AI summary (POST /home/summary).
//! Covers the audience gate (flag OFF -> 403 even for an admin; flag ON +
//! default "everyone" -> an ordinary member is summarized; flag ON + "staff" ->
//! member 403 while admin passes) and the empty-workspace race (nothing unread
//! -> a friendly caught-up fragment, not an error). Harness mirrors
//! routes_ai_gate.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct Setup {
    app: Router,
    chat: sqlx::SqlitePool,
    settings: sqlx::SqlitePool,
    admin: String,
    member: String,
    admin_session: String,
    member_session: String,
}

async fn setup() -> Setup {
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member = db::auth::create_user(&auth, "member", "h").await.unwrap();
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
        llm_client: Some(Arc::new(lets_chat::llm::MockLlmClient {
            canned: "## Summary\n- All good.\n\n## Action items\n- None.".into(),
        })),
        embedding_client: None,
    };
    Setup {
        app: routes::build_router(state),
        chat,
        settings,
        admin,
        member,
        admin_session,
        member_session,
    }
}

/// Seed an unread message in General (room 1) authored by `author`, so any other
/// member has something to summarize.
async fn seed_unread(chat: &sqlx::SqlitePool, author: &str) {
    db::chat::insert_message(chat, 1, author, "ship the release by friday")
        .await
        .unwrap();
}

async fn post_home_summary(app: &Router, session: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/home/summary")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

/// GET the Home dashboard (`?home=1` forces it past the last-visited redirect)
/// and return (status, body). Used to smoke-test that the composed template and
/// every new `|t` key render at runtime, which askama's compile step does not
/// check for the runtime Fluent lookups.
async fn get_dashboard(app: &Router, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/?home=1")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn set_flag(settings: &sqlx::SqlitePool, on: bool) {
    db::settings::set_setting(settings, "llm_enabled", if on { "true" } else { "false" })
        .await
        .unwrap();
}

async fn set_staff_only(settings: &sqlx::SqlitePool, staff_only: bool) {
    let value = if staff_only { "staff" } else { "everyone" };
    db::settings::set_setting(settings, "llm_audience", value)
        .await
        .unwrap();
}

#[tokio::test]
async fn flag_off_forbids_even_a_site_admin() {
    let s = setup().await;
    seed_unread(&s.chat, &s.admin).await;
    set_flag(&s.settings, false).await;
    assert_eq!(
        post_home_summary(&s.app, &s.admin_session).await,
        StatusCode::FORBIDDEN,
        "flag off must 403 the workspace summary, even for an admin"
    );
}

#[tokio::test]
async fn flag_on_everyone_audience_summarizes_for_a_member() {
    let s = setup().await;
    // Unread authored by the admin: the member (not the author) has it unread.
    seed_unread(&s.chat, &s.admin).await;
    set_flag(&s.settings, true).await;
    assert_eq!(
        post_home_summary(&s.app, &s.member_session).await,
        StatusCode::OK,
        "with the default 'everyone' audience a plain member gets a workspace summary"
    );
}

#[tokio::test]
async fn flag_on_staff_audience_forbids_a_member_but_allows_admin() {
    let s = setup().await;
    // Author with the member so the admin (the reader who must pass) has unread.
    seed_unread(&s.chat, &s.member).await;
    set_flag(&s.settings, true).await;
    set_staff_only(&s.settings, true).await;
    assert_eq!(
        post_home_summary(&s.app, &s.member_session).await,
        StatusCode::FORBIDDEN,
        "the 'staff' audience must refuse an unprivileged member"
    );
    assert_eq!(
        post_home_summary(&s.app, &s.admin_session).await,
        StatusCode::OK,
        "an admin stays within the 'staff' audience"
    );
}

#[tokio::test]
async fn dashboard_renders_greeting_quick_actions_and_quiet_strip() {
    // LC-706: a member with an accessible room (General) but nothing unread sees
    // the composed dashboard: greeting, quick actions, and the collapsed
    // "all caught up" strip in place of empty section cards. This exercises every
    // new `|t` key, so a missing locale entry surfaces as a failed assertion.
    let s = setup().await;
    let (status, body) = get_dashboard(&s.app, &s.member_session).await;
    assert_eq!(status, StatusCode::OK, "dashboard must render for a member");
    assert!(
        body.contains("data-lc-greeting") && body.contains("Welcome back,"),
        "greeting with its client-side enhancement hook must render"
    );
    assert!(
        body.contains("New message") && body.contains("Jump to a room"),
        "quick-action launchpad must render"
    );
    assert!(
        body.contains("lc-home-quiet") && body.contains("All caught up"),
        "empty sections must collapse into the quiet strip"
    );
}

#[tokio::test]
async fn nothing_unread_returns_a_caught_up_fragment_not_an_error() {
    let s = setup().await;
    // No seeded message: the admin has nothing unread. The gate passes, but the
    // handler returns the caught-up fragment (200) rather than an error.
    set_flag(&s.settings, true).await;
    assert_eq!(
        post_home_summary(&s.app, &s.admin_session).await,
        StatusCode::OK,
        "an empty workspace must render the caught-up note, not 4xx"
    );
}
