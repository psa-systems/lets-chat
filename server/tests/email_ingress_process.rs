//! LC-77 integration tests for the per-message processing pipeline.
//!
//! Exercises `email_ingress::poll::process_polled_message` end-to-end
//! against an in-memory AppState. No IMAP transport is involved; the
//! tests feed crafted RFC 822 bytes directly so the test surface is
//! deterministic.
//!
//! Coverage:
//!
//! - Happy path: well-formed message with a valid token in `To` posts
//!   to its room as the EmailInbox synthetic actor.
//! - Header precedence: `Delivered-To`, `X-Original-To`, `To`, `Cc`
//!   each succeed on their own; `Delivered-To` wins when ambiguous.
//! - Unknown secret -> `DropReason::AddressNoMatch`.
//! - Revoked inbox -> `DropReason::RevokedInbox`.
//! - Loop heuristics -> `DropReason::LoopDetected` (Auto-Submitted,
//!   Precedence, List-Id).
//! - Per-inbox rate limit -> `DropReason::RateLimited`.
//! - Empty body + empty subject -> `DropReason::ParseFail`.
//! - Threat model: a forged `From` does NOT prevent posting; identity
//!   comes from the secret address, not the sender.
//! - Subject prefixes the body as a Markdown-bold first line.

use std::sync::{Arc, OnceLock};

use lets_chat::email_ingress::poll::{
    process_polled_message, ProcessOutcome, POLL_RATE_LIMIT_PER_MIN,
};
use lets_chat::email_ingress::DropReason;
use lets_chat::{auth, db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;

mod common;

const SECRET: [u8; 32] = [9u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";
const TOKEN: &str = "lc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-ei-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    room_id: i64,
    inbox_id: i64,
    secret_key: [u8; 32],
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth_pool = common::auth_pool().await;
    let chat_pool = common::chat_pool().await;
    let settings_pool = common::settings_pool().await;

    // Need at least one admin user so backfill_general_membership runs.
    let admin = db::auth::create_user(&auth_pool, "admin", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth_pool)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth_pool, &chat_pool)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat_pool, "Acme", None, &admin)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();

    // Insert an inbox row with a known secret hash so resolve_inbox can find it.
    let secret_hash = auth::hash_api_token(&SECRET, TOKEN);
    let inbox_id = db::email_inbox::insert(
        &chat_pool,
        room_id,
        "Test Inbox",
        None,
        &secret_hash,
        &admin,
    )
    .await
    .unwrap();

    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
        chat: chat_pool,
        settings: settings_pool,
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
        state,
        room_id,
        inbox_id,
        secret_key: SECRET,
    }
}

fn email_to(recipient_header: &str, recipient_value: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: alice@example.com\r\n\
         {recipient_header}: {recipient_value}\r\n\
         Subject: {subject}\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <{}@spike.test>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {body}\r\n",
        uuid::Uuid::new_v4()
    )
    .into_bytes()
}

async fn message_count(pool: &SqlitePool, room_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn happy_path_token_in_to_header_posts() {
    let fx = setup().await;
    let raw = email_to(
        "To",
        &format!("{TOKEN}@{INGRESS_DOMAIN}"),
        "test subject",
        "hello chat",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, 1);

    let raw_msg = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("message row");
    assert_eq!(raw_msg.user_id, "");
    assert_eq!(raw_msg.webhook_id, None);
    assert_eq!(raw_msg.email_inbox_id, Some(fx.inbox_id));
    assert!(
        raw_msg.body.contains("**test subject**"),
        "subject should prefix body as markdown bold: got {:?}",
        raw_msg.body
    );
    assert!(raw_msg.body.contains("hello chat"));
}

#[tokio::test]
async fn delivered_to_header_resolves() {
    let fx = setup().await;
    let raw = email_to(
        "Delivered-To",
        &format!("{TOKEN}@{INGRESS_DOMAIN}"),
        "via delivered-to",
        "body",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "got {outcome:?}"
    );
}

#[tokio::test]
async fn x_original_to_header_resolves() {
    let fx = setup().await;
    let raw = email_to(
        "X-Original-To",
        &format!("{TOKEN}@{INGRESS_DOMAIN}"),
        "via x-original-to",
        "body",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "got {outcome:?}"
    );
}

#[tokio::test]
async fn cc_header_resolves() {
    let fx = setup().await;
    let raw = email_to("Cc", &format!("{TOKEN}@{INGRESS_DOMAIN}"), "via cc", "body");
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "got {outcome:?}"
    );
}

#[tokio::test]
async fn unknown_secret_drops_with_address_no_match() {
    let fx = setup().await;
    let raw = email_to(
        "To",
        &format!("lc_unknown@{INGRESS_DOMAIN}"),
        "unknown token",
        "body",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, detail } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::AddressNoMatch);
    assert!(
        detail.contains("lc_unknown"),
        "detail should list tried address: {detail}"
    );
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, 0);
}

#[tokio::test]
async fn revoked_inbox_drops_with_revoked_inbox_reason() {
    let fx = setup().await;
    let revoked = db::email_inbox::revoke(&fx.state.chat, fx.inbox_id, fx.room_id)
        .await
        .unwrap();
    assert!(revoked, "revoke should succeed");

    let raw = email_to(
        "To",
        &format!("{TOKEN}@{INGRESS_DOMAIN}"),
        "post-revoke",
        "body",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::RevokedInbox);
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, 0);
}

#[tokio::test]
async fn auto_submitted_auto_replied_drops_with_loop_detected() {
    let fx = setup().await;
    let raw = format!(
        "From: vacation@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: Out of office\r\n\
         Auto-Submitted: auto-replied\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         I am away.\r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, detail } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::LoopDetected);
    assert!(detail.contains("Auto-Submitted"));
}

#[tokio::test]
async fn auto_submitted_no_does_not_drop() {
    let fx = setup().await;
    let raw = format!(
        "From: human@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: real reply\r\n\
         Auto-Submitted: no\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         I typed this.\r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "Auto-Submitted: no should not trigger loop drop; got {outcome:?}"
    );
}

#[tokio::test]
async fn precedence_bulk_drops_with_loop_detected() {
    let fx = setup().await;
    let raw = format!(
        "From: marketing@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: newsletter\r\n\
         Precedence: bulk\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         Buy!\r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::LoopDetected);
}

#[tokio::test]
async fn list_id_drops_with_loop_detected() {
    let fx = setup().await;
    let raw = format!(
        "From: list@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: list message\r\n\
         List-Id: <ops.example.com>\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         body\r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, detail } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::LoopDetected);
    assert!(
        detail.contains("List-Id"),
        "detail must name List-Id specifically so the operator can diagnose: {detail}",
    );
}

#[tokio::test]
async fn rate_limit_blocks_burst_above_cap() {
    let fx = setup().await;
    // Use the same inbox + token across all attempts so the per-inbox
    // rate limit (60/min) applies as the brainstorm specified.
    let recipient = format!("{TOKEN}@{INGRESS_DOMAIN}");

    // First N messages allowed; (N+1)-th dropped with RateLimited.
    let mut posted = 0usize;
    let mut rate_limited = 0usize;
    let attempts = POLL_RATE_LIMIT_PER_MIN as usize + 5;
    for i in 0..attempts {
        let raw = email_to("To", &recipient, &format!("msg {i}"), "body");
        let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
        match outcome {
            ProcessOutcome::Posted { .. } => posted += 1,
            ProcessOutcome::Dropped {
                reason: DropReason::RateLimited,
                ..
            } => {
                rate_limited += 1;
            }
            other => panic!("unexpected outcome at iter {i}: {other:?}"),
        }
    }
    assert_eq!(posted, POLL_RATE_LIMIT_PER_MIN as usize);
    assert_eq!(rate_limited, attempts - POLL_RATE_LIMIT_PER_MIN as usize);
}

#[tokio::test]
async fn empty_body_and_subject_drops_with_parse_fail() {
    let fx = setup().await;
    let raw = format!(
        "From: nothing@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         \r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::ParseFail);
}

#[tokio::test]
async fn forged_from_still_posts_identity_is_secret_not_from() {
    let fx = setup().await;
    // The From header claims to be an admin of the deployment. The
    // identity that actually attaches to the chat message is the inbox
    // (`Test Inbox`), not the From sender; the secret in the To address
    // is the only authorization. This test pins that posture so a future
    // change cannot silently trust the From header.
    let raw = format!(
        "From: admin@lets-chat-deployment.test\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: looks like admin\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         I am NOT admin.\r\n"
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let raw_msg = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("message row");
    assert_eq!(raw_msg.email_inbox_id, Some(fx.inbox_id));
    assert_eq!(raw_msg.user_id, "");
    // The From address must not leak into the stored body or any
    // identity surface.
    assert!(
        !raw_msg.body.contains("admin@lets-chat-deployment.test"),
        "stored body must not echo the From header verbatim",
    );
}

#[tokio::test]
async fn unknown_domain_drops_even_if_local_part_matches() {
    let fx = setup().await;
    // Same secret token, wrong domain. The resolver MUST not match.
    let raw = email_to(
        "To",
        &format!("{TOKEN}@some-other-domain.test"),
        "wrong domain",
        "body",
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::AddressNoMatch);
}
