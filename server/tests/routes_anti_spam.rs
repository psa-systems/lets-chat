//! LC-94 integration: anti-spam rate limits, link filter, honeypot,
//! quarantine review.
//!
//! Each subsystem has its own block of tests so a regression points
//! straight at the broken defense.

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
        let p = std::env::temp_dir().join(format!("lc-anti-spam-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

#[allow(dead_code)]
struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    member_id: String,
    auth: SqlitePool,
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
        llm_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        member_id,
        auth,
        chat,
        settings,
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

// LC-22 cutover: send_with_ip / send_with_ip_full helpers deleted with the
// per-IP register/login/2FA rate-limit tests they fed.

// ── Rate limits ───────────────────────────────────────────────────────────

#[tokio::test]
async fn message_rate_limit_returns_429_after_cap() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "rate_limit_messages", "3")
        .await
        .unwrap();
    for i in 0..3 {
        let (status, body) = send(
            &t.app,
            Some(&t.member_session),
            Method::POST,
            "/room/1/messages",
            &format!("body=msg{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "msg {i} failed: {body}");
    }
    let (status, body) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        "body=overflow",
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("too quickly"), "body: {body}");
}

#[tokio::test]
async fn message_rate_limit_disabled_when_cap_is_zero() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "rate_limit_messages", "0")
        .await
        .unwrap();
    // Many sends in a row all pass.
    for i in 0..10 {
        let (status, _) = send(
            &t.app,
            Some(&t.member_session),
            Method::POST,
            "/room/1/messages",
            &format!("body=msg{i}"),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
}

// ── Message length cap (LC-153) ───────────────────────────────────────────

#[tokio::test]
async fn message_over_length_cap_returns_400() {
    let t = app().await;
    // 16_001 chars: one past MAX_MESSAGE_CHARS.
    let huge = "x".repeat(16_001);
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        &format!("body={huge}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn message_at_length_cap_is_accepted() {
    let t = app().await;
    let at_limit = "y".repeat(16_000);
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        &format!("body={at_limit}"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// LC-358: the site-wide message rate-limit must be clamped, like the
// per-enclave burst, so an operator typo cannot store an absurd value.
#[cfg(feature = "standalone")]
#[tokio::test]
async fn rate_limit_messages_is_clamped_to_max() {
    let t = app().await;
    let (status, body) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        "/admin/anti-spam",
        "rate_limit_messages=99999",
    )
    .await;
    assert!(
        status.is_redirection(),
        "expected redirect, got {status}: {body}"
    );
    let stored = db::settings::get_setting(&t.settings, "rate_limit_messages")
        .await
        .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("10000"),
        "should clamp to the 10000 ceiling"
    );
}

// ── Honeypot + register ───────────────────────────────────────────────────

// LC-22 cutover: registration / login / 2FA rate-limit tests deleted with the
// password path. The site-wide rate_limit_messages knob (exercised above) is
// the only anti-spam surface lets-chat still owns; per-IP login throttling is
// upstream at Bunyip.

// LC-94 follow-up TODO: end-to-end /forgot rate-limit test needs a
// stub `Mailer` in the test AppState. The current AppState literal
// hard-codes `mailer: None`, which makes `state.mail_available()`
// false and 404s before the rate-limit gate runs. Wiring a
// MockMailer (mirror the MockPushClient pattern) would let an
// assertion actually exercise the 429 path; until then the
// `client_ip_for_rate_limit` + `rate_limits.check` plumbing is
// covered by the unit tests in rate_limit.rs.

// ── Link filter ───────────────────────────────────────────────────────────

#[tokio::test]
async fn link_filter_block_refuses_message() {
    let t = app().await;
    db::anti_spam::insert_rule(
        &t.chat,
        "*.evil.com",
        db::anti_spam::FilterAction::Block,
        "admin",
    )
    .await
    .unwrap();
    let (status, body) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        "body=visit https://x.evil.com/path now",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("disallowed"), "body: {body}");
}

#[tokio::test]
async fn link_filter_quarantine_hides_message_until_approved() {
    let t = app().await;
    db::anti_spam::insert_rule(
        &t.chat,
        "*.spammy.net",
        db::anti_spam::FilterAction::Quarantine,
        "admin",
    )
    .await
    .unwrap();
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        "body=see https://foo.spammy.net",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "post returns 204 (LC-228)");
    // The message exists but is invisible to readers.
    let visible = db::chat::list_messages(&t.chat, 1).await.unwrap();
    assert!(
        visible.iter().all(|m| !m.body.contains("spammy.net")),
        "quarantined message must not appear in list_messages"
    );
    // Admin queue surfaces it.
    let pending = db::anti_spam::list_pending_quarantine(&t.chat)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    let mid = pending[0].message_id;
    assert_eq!(pending[0].matched_pattern, "*.spammy.net");

    // Approve flips visibility and clears the queue.
    db::anti_spam::approve_quarantine(&t.chat, mid, &t.admin_id)
        .await
        .unwrap();
    let visible = db::chat::list_messages(&t.chat, 1).await.unwrap();
    assert!(visible.iter().any(|m| m.body.contains("spammy.net")));
    let pending = db::anti_spam::list_pending_quarantine(&t.chat)
        .await
        .unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn link_filter_warn_passes_through_and_audit_logs() {
    let t = app().await;
    db::anti_spam::insert_rule(
        &t.chat,
        "watchme.org",
        db::anti_spam::FilterAction::Warn,
        "admin",
    )
    .await
    .unwrap();
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        "body=check https://watchme.org/x",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let visible = db::chat::list_messages(&t.chat, 1).await.unwrap();
    assert!(visible.iter().any(|m| m.body.contains("watchme.org")));
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert!(
        actions.iter().any(|a| a.action == "link_warn"),
        "warn must audit-log"
    );
}

#[tokio::test]
async fn link_filter_disabled_lets_block_rule_pass() {
    let t = app().await;
    db::settings::set_setting(&t.settings, "link_filter_enabled", "false")
        .await
        .unwrap();
    db::anti_spam::insert_rule(
        &t.chat,
        "*.evil.com",
        db::anti_spam::FilterAction::Block,
        "admin",
    )
    .await
    .unwrap();
    let (status, _) = send(
        &t.app,
        Some(&t.member_session),
        Method::POST,
        "/room/1/messages",
        "body=https://x.evil.com",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ── Admin pages (standalone-only) ─────────────────────────────────────────

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_anti_spam_page_persists_settings_and_audits() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        "/admin/anti-spam",
        "rate_limit_messages=10&rate_limit_registrations=5&rate_limit_password_resets=3&link_filter_enabled=1",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        db::settings::get_setting(&t.settings, "rate_limit_messages")
            .await
            .unwrap()
            .as_deref(),
        Some("10"),
    );
    // honeypot was OFF in the form (omitted checkbox) -> "false".
    assert_eq!(
        db::settings::get_setting(&t.settings, "honeypot_enabled")
            .await
            .unwrap()
            .as_deref(),
        Some("false"),
    );
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert!(actions.iter().any(|a| a.action == "anti_spam_settings"));
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_link_filter_add_and_delete_round_trip() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        "/admin/link-filter",
        "pattern=test.example&action=block",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let rules = db::anti_spam::list_rules(&t.chat).await.unwrap();
    let rule = rules
        .iter()
        .find(|r| r.pattern == "test.example")
        .expect("rule inserted");
    let id = rule.id;
    let (status, _) = send(
        &t.app,
        Some(&t.admin_session),
        Method::POST,
        &format!("/admin/link-filter/{id}/delete"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let rules = db::anti_spam::list_rules(&t.chat).await.unwrap();
    assert!(!rules.iter().any(|r| r.pattern == "test.example"));
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn non_admin_cannot_open_anti_spam_pages() {
    let t = app().await;
    for path in [
        "/admin/anti-spam",
        "/admin/link-filter",
        "/admin/quarantine",
    ] {
        let (status, _) = send(&t.app, Some(&t.member_session), Method::GET, path, "").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "path: {path}");
    }
}
