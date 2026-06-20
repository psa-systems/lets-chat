//! LC-77-MID-DEDUP commit 2: integration tests for the wired dedup
//! defense in `process_polled_message`.
//!
//! The dedup is keyed on the RFC 5322 Message-ID header, hashed under
//! `LETS_CHAT_SECRET_KEY`. A successful post records the hash; a
//! re-fetch of the same UID (the crash-between-process-and-STORE-Seen
//! race) drops with `DropReason::Duplicate` instead of double-posting.

use std::sync::{Arc, OnceLock};

use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::{auth, db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;

mod common;

const SECRET: [u8; 32] = [33u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";
const TOKEN: &str = "lc_dedup_token_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-dedup-path-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    room_id: i64,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth_pool = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let admin = db::auth::create_user(&auth_pool, "admin", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth_pool)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth_pool, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();

    let secret_hash = auth::hash_api_token(&SECRET, TOKEN);
    db::email_inbox::insert(&chat, room_id, "Test Inbox", None, &secret_hash, &admin)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
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
    };
    Fixture { state, room_id }
}

fn email_with_message_id(message_id: Option<&str>, body: &str) -> Vec<u8> {
    let mid_header = match message_id {
        Some(m) => format!("Message-ID: {m}\r\n"),
        None => String::new(),
    };
    format!(
        "From: alice@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: subject\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         {mid_header}\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {body}\r\n",
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
async fn second_process_of_same_message_id_drops_with_duplicate() {
    // Simulates the crash-between-process-and-STORE-Seen race:
    // `process_polled_message` runs twice with the same raw bytes
    // (same UID would be re-fetched on the next tick if STORE failed).
    // First call posts; second call must drop with Duplicate.
    let fx = setup().await;
    let raw = email_with_message_id(Some("<dedup-1@example.com>"), "first body");

    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "first call must post, got {outcome:?}",
    );
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, 1);

    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("second call must drop, got {outcome:?}");
    };
    assert_eq!(reason.as_str(), "duplicate");
    assert_eq!(
        message_count(&fx.state.chat, fx.room_id).await,
        1,
        "duplicate must NOT insert a second message row",
    );
}

#[tokio::test]
async fn two_distinct_message_ids_both_post() {
    // Regression: the dedup must be keyed correctly; two different
    // Message-IDs must both post regardless of arrival order.
    let fx = setup().await;

    let raw_a = email_with_message_id(Some("<dedup-a@example.com>"), "alpha");
    let raw_b = email_with_message_id(Some("<dedup-b@example.com>"), "bravo");

    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw_a).await,
        ProcessOutcome::Posted { .. }
    ));
    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw_b).await,
        ProcessOutcome::Posted { .. }
    ));
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, 2);
}

#[tokio::test]
async fn message_without_message_id_falls_back_to_at_least_once() {
    // RFC 5322 says Message-ID SHOULD be set, but not MUST. A message
    // without one cannot be deduped (we have nothing to key on);
    // process_polled_message must post both calls, matching v1
    // at-least-once behavior. This is documented in the operator
    // docs as the dedup's known gap.
    let fx = setup().await;

    let raw = email_with_message_id(None, "no message-id");
    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await,
        ProcessOutcome::Posted { .. }
    ));
    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await,
        ProcessOutcome::Posted { .. }
    ));
    assert_eq!(
        message_count(&fx.state.chat, fx.room_id).await,
        2,
        "without a Message-ID the dedup cannot trigger; both posts land",
    );
}

#[tokio::test]
async fn duplicate_check_runs_before_actor_so_failed_resolve_still_dedupes_on_replay() {
    // Confirms ordering: the dedup check runs BEFORE the resolver, so
    // a Message-ID that ALREADY posted in a prior tick drops with
    // `duplicate` even if the address has since been revoked. This is
    // the intended posture: once we've posted a message, replays of
    // that exact wire bytes should never produce a second post,
    // regardless of what else has changed in the meantime.
    let fx = setup().await;
    let raw = email_with_message_id(Some("<ordering-check@example.com>"), "body");

    // First pass posts normally.
    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await,
        ProcessOutcome::Posted { .. }
    ));

    // Revoke the inbox between calls. A v1 re-fetch would now drop
    // with `revoked_inbox`; the dedup intercepts first and drops with
    // `duplicate` instead. Both are non-posting; the assertion is on
    // the specific drop reason so an operator log clearly shows the
    // dedup is doing its job rather than the inbox-state change.
    sqlx::query("UPDATE email_inboxes SET revoked_at = datetime('now')")
        .execute(&fx.state.chat)
        .await
        .unwrap();

    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(
        reason.as_str(),
        "duplicate",
        "dedup check must run before resolve so a replay drops as duplicate not revoked_inbox",
    );
}
