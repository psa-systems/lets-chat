//! LC-77 admin lifecycle tests for per-room email inboxes.
//!
//! Mirrors `routes_webhooks.rs` in shape (cookie-auth admin user, in-memory
//! AppState, axum tower::ServiceExt::oneshot for each request) but exercises
//! the create / list / revoke flow on `/room/{id}/email-inboxes`.
//!
//! Plus one end-to-end test that creates an inbox via the admin HTTP route,
//! parses the secret out of the rendered HTML, and then drives
//! `email_ingress::poll::process_polled_message` with a crafted RFC 822
//! message addressed to that secret. Proves the create -> resolve -> post
//! chain is wired correctly all the way from the HTTP form to the database
//! row that the IMAP poll loop would surface.

use std::sync::{Arc, OnceLock};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::{db, db::imap_config::ImapConfig, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

mod common;

const SECRET: [u8; 32] = [11u8; 32];
const INGRESS_DOMAIN: &str = "ingress.example.com";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-eib-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    admin_session: String,
    user_session: String,
    room: i64,
    chat: SqlitePool,
    state: AppState,
}

async fn app_with_ingress_domain(domain: Option<&str>) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin).await.unwrap();

    let regular = db::auth::create_user(&auth, "regular", "h").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&regular)
        .execute(&auth)
        .await
        .unwrap();
    let user_session = db::auth::create_session(&auth, &regular).await.unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();

    // Optionally seed an imap_inbox_config row so the create handler can
    // surface an ingress_domain to the rendered address.
    if let Some(d) = domain {
        let cfg = ImapConfig {
            host: "imap.example.com".into(),
            port: 993,
            tls: true,
            username: "mailer".into(),
            password: "secret".into(),
            folder: "INBOX".into(),
            ingress_domain: Some(d.to_string()),
            enabled: false,
            dead_letter_folder: None,
        };
        db::imap_config::write(&settings, &SECRET, &cfg)
            .await
            .unwrap();
    }

    let bg = lets_chat::bg::spawn(auth.clone());
    let chat_for_test = chat.clone();
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
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
    let app = routes::build_router(state.clone());
    TestApp {
        app,
        admin_session,
        user_session,
        room,
        chat: chat_for_test,
        state,
    }
}

async fn app() -> TestApp {
    app_with_ingress_domain(Some(INGRESS_DOMAIN)).await
}

async fn post_create(
    app: &Router,
    session: &str,
    room_id: i64,
    name: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/email-inboxes"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::from(format!("name={name}&avatar_url=")))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get_list(app: &Router, session: &str, room_id: i64) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/room/{room_id}/email-inboxes"))
        .header(header::COOKIE, format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Extract the `<token>@<domain>` address from the create-flow response.
/// The template renders the address inside a `<code>` block immediately
/// after the "Inbox created. Copy its address now" success banner.
fn extract_new_address(html: &str) -> String {
    let marker = "Copy its address now";
    let after = html.find(marker).expect("success banner present");
    let tail = &html[after..];
    let code_open = tail.find("<code").expect("code element present");
    let gt = tail[code_open..].find('>').unwrap() + code_open + 1;
    let code_close = tail[gt..].find("</code>").unwrap() + gt;
    tail[gt..code_close].trim().to_string()
}

#[tokio::test]
async fn create_email_inbox_reveals_address_once() {
    let t = app().await;
    let (status, body) = post_create(&t.app, &t.admin_session, t.room, "Alerts").await;
    assert_eq!(status, StatusCode::OK);
    let address = extract_new_address(&body);
    assert!(
        address.ends_with(&format!("@{INGRESS_DOMAIN}")),
        "address should end with @{INGRESS_DOMAIN}: {address}",
    );
    let (local, _) = address.split_once('@').unwrap();
    assert!(
        local.starts_with("lc_"),
        "token should carry the lc_ prefix: {local}",
    );
    // The plaintext token must NOT appear in any subsequent list-page render.
    let (list_status, list_body) = get_list(&t.app, &t.admin_session, t.room).await;
    assert_eq!(list_status, StatusCode::OK);
    assert!(
        !list_body.contains(local),
        "list page must never re-render the plaintext token",
    );
    // And the address must NOT round-trip from the database either: only
    // the hash is stored.
    let hash = lets_chat::auth::hash_api_token(&SECRET, local);
    let auth = db::email_inbox::find_by_secret_hash(&t.chat, &hash)
        .await
        .unwrap()
        .expect("inbox row by hash");
    assert_eq!(auth.room_id, t.room);
    assert_eq!(auth.name, "Alerts");
}

#[tokio::test]
async fn list_includes_the_created_inbox_with_active_status() {
    let t = app().await;
    let (_, _) = post_create(&t.app, &t.admin_session, t.room, "Pager").await;
    let (status, body) = get_list(&t.app, &t.admin_session, t.room).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Pager"), "list should show the inbox name");
    assert!(body.contains("active"), "list should mark new inbox active");
    assert!(
        !body.contains("revoked"),
        "list should not mark a fresh inbox as revoked",
    );
}

#[tokio::test]
async fn revoke_flips_inbox_to_revoked() {
    let t = app().await;
    let (_, _) = post_create(&t.app, &t.admin_session, t.room, "Pager").await;
    let inbox_id: i64 = sqlx::query_scalar("SELECT id FROM email_inboxes WHERE room_id = ?")
        .bind(t.room)
        .fetch_one(&t.chat)
        .await
        .unwrap();

    let revoke_req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/room/{}/email-inboxes/{}/revoke",
            t.room, inbox_id,
        ))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let revoke_res = t.app.clone().oneshot(revoke_req).await.unwrap();
    assert!(
        revoke_res.status().is_redirection(),
        "revoke should redirect back to the inbox list",
    );

    let (status, body) = get_list(&t.app, &t.admin_session, t.room).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("revoked"), "list should mark revoked status");
}

#[tokio::test]
async fn non_moderator_forbidden_from_admin_routes() {
    let t = app().await;
    let (status, _) = post_create(&t.app, &t.user_session, t.room, "Sneaky").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (list_status, _) = get_list(&t.app, &t.user_session, t.room).await;
    assert_eq!(list_status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_blocked_when_ingress_domain_unset() {
    let t = app_with_ingress_domain(None).await;
    let (status, body) = post_create(&t.app, &t.admin_session, t.room, "Alerts").await;
    // The handler renders the form page with an error banner rather than
    // returning 4xx; the failure mode is operator-fixable from the UI.
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("ingress domain"),
        "error banner should name the missing ingress domain setting; got: {}",
        body.lines()
            .filter(|l| l.to_lowercase().contains("error") || l.to_lowercase().contains("ingress"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    // No row was inserted.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_inboxes WHERE room_id = ?")
        .bind(t.room)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn end_to_end_create_then_poll_processes_real_mail() {
    // The load-bearing integration test for this PR. Confirms the
    // admin-create HTTP flow ends with a row that the IMAP poll loop's
    // resolver can find by hashing the local part of an incoming address.
    let t = app().await;
    let (status, body) = post_create(&t.app, &t.admin_session, t.room, "Pager").await;
    assert_eq!(status, StatusCode::OK);
    let address = extract_new_address(&body);

    let raw = format!(
        "From: monitoring@example.com\r\n\
         To: {address}\r\n\
         Subject: disk full on prod\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <e2e@spike.test>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         /var is 95% full.\r\n",
    )
    .into_bytes();

    let outcome = process_polled_message(&t.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let msg = db::chat::get_message(&t.chat, message_id)
        .await
        .unwrap()
        .expect("message row");
    assert_eq!(msg.room_id, t.room);
    assert_eq!(msg.user_id, "");
    assert!(msg.email_inbox_id.is_some());
    assert!(msg.body.contains("**disk full on prod**"));
    assert!(msg.body.contains("/var is 95% full"));
}

#[tokio::test]
async fn revoked_inbox_no_longer_accepts_polled_mail() {
    let t = app().await;
    let (_, body) = post_create(&t.app, &t.admin_session, t.room, "Pager").await;
    let address = extract_new_address(&body);
    let inbox_id: i64 = sqlx::query_scalar("SELECT id FROM email_inboxes WHERE room_id = ?")
        .bind(t.room)
        .fetch_one(&t.chat)
        .await
        .unwrap();

    // Revoke via the HTTP route, mirroring how an admin would do it.
    let revoke_req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/room/{}/email-inboxes/{}/revoke",
            t.room, inbox_id,
        ))
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let _ = t.app.clone().oneshot(revoke_req).await.unwrap();

    let raw = format!(
        "From: a@example.com\r\n\
         To: {address}\r\n\
         Subject: post-revoke ping\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         hi\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&t.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped after revoke, got {outcome:?}");
    };
    assert_eq!(reason, lets_chat::email_ingress::DropReason::RevokedInbox);
}
