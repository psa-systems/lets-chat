//! LC-77-REPLY commit 2d: threat-model integration tests for the
//! reply-by-email surface.
//!
//! Covers the posting-gate matrix that mirrors the HTTP `post_message`
//! path, plus reply-token replay defenses, forged-From robustness, and
//! hostile-input handling for the quote/signature strip.

use std::sync::{Arc, OnceLock};

use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::{db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;

mod common;

const SECRET: [u8; 32] = [42u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-reply-threat-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    alice_id: String,
    bob_id: String,
    room_id: i64,
    dm_room: i64,
    original_id: i64,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bob_id)
        .execute(&auth)
        .await
        .unwrap();

    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &alice_id)
        .await
        .unwrap();
    db::enclave::add_member(
        &chat,
        eid,
        &bob_id,
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let room_id = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let original_id = db::chat::insert_message(&chat, room_id, &alice_id, "hello @bob")
        .await
        .unwrap();

    let dm = db::chat::create_dm_room(&chat, "alice-bob", &alice_id, &bob_id)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
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
    };
    Fixture {
        state,
        alice_id,
        bob_id,
        room_id,
        dm_room: dm.id,
        original_id,
    }
}

fn email_reply(from_addr: &str, token: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from_addr}\r\n\
         To: reply-{token}@{INGRESS_DOMAIN}\r\n\
         Subject: Re: [lets-chat] alice mentioned you in #ops\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <{}@spike.test>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {body}\r\n",
        uuid::Uuid::new_v4()
    )
    .into_bytes()
}

async fn mint(pool: &SqlitePool, user_id: &str, message_id: i64) -> String {
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(pool, &token, user_id, message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();
    token
}

async fn message_count(pool: &SqlitePool, room_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn banned_user_reply_is_dropped_and_token_not_consumed() {
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    sqlx::query("UPDATE users SET is_banned = 1 WHERE id = ?")
        .bind(&fx.bob_id)
        .execute(&fx.state.auth)
        .await
        .unwrap();

    let raw = email_reply("bob@example.com", &token, "hostile reply");
    let pre = message_count(&fx.state.chat, fx.room_id).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { .. } = outcome else {
        panic!("a banned user's reply must drop, got {outcome:?}");
    };
    assert_eq!(
        message_count(&fx.state.chat, fx.room_id).await,
        pre,
        "no message row may be inserted for a banned user's reply",
    );
    assert!(
        db::reply_tokens::resolve(&fx.state.chat, &token)
            .await
            .unwrap()
            .is_some(),
        "a gate-failure drop must NOT consume the token (so the user can recover after un-ban)",
    );
}

#[tokio::test]
async fn muted_user_reply_is_dropped() {
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    sqlx::query("UPDATE users SET is_muted = 1 WHERE id = ?")
        .bind(&fx.bob_id)
        .execute(&fx.state.auth)
        .await
        .unwrap();

    let raw = email_reply("bob@example.com", &token, "silenced reply");
    let pre = message_count(&fx.state.chat, fx.room_id).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(matches!(outcome, ProcessOutcome::Dropped { .. }));
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, pre);
}

#[tokio::test]
async fn user_removed_from_enclave_cannot_reply_into_that_room() {
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;

    // Look up the enclave for the room, then remove bob.
    let enclave_id: i64 = sqlx::query_scalar("SELECT enclave_id FROM rooms WHERE id = ?")
        .bind(fx.room_id)
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    sqlx::query("DELETE FROM enclave_members WHERE enclave_id = ? AND user_id = ?")
        .bind(enclave_id)
        .bind(&fx.bob_id)
        .execute(&fx.state.chat)
        .await
        .unwrap();

    let raw = email_reply("bob@example.com", &token, "trying to post anyway");
    let pre = message_count(&fx.state.chat, fx.room_id).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(matches!(outcome, ProcessOutcome::Dropped { .. }));
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, pre);
}

#[tokio::test]
async fn moderators_only_room_drops_non_moderator_reply() {
    let fx = setup().await;
    db::chat::set_room_posting_policy(&fx.state.chat, fx.room_id, "moderators_only")
        .await
        .unwrap();
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    let raw = email_reply("bob@example.com", &token, "regular member reply");
    let pre = message_count(&fx.state.chat, fx.room_id).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(matches!(outcome, ProcessOutcome::Dropped { .. }));
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, pre);
}

#[tokio::test]
async fn dm_block_drops_reply_silently() {
    let fx = setup().await;
    // Alice DMs bob; the original message used for token mint lives in the DM.
    let dm_original = db::chat::insert_message(&fx.state.chat, fx.dm_room, &fx.alice_id, "hi bob")
        .await
        .unwrap();
    let token = mint(&fx.state.chat, &fx.bob_id, dm_original).await;
    db::auth::block_user(&fx.state.auth, &fx.alice_id, &fx.bob_id)
        .await
        .unwrap();

    let raw = email_reply("bob@example.com", &token, "even with block");
    let pre = message_count(&fx.state.chat, fx.dm_room).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(matches!(outcome, ProcessOutcome::Dropped { .. }));
    assert_eq!(message_count(&fx.state.chat, fx.dm_room).await, pre);
}

#[tokio::test]
async fn forged_from_does_not_change_identity() {
    // The threat model: an attacker who steals/forwards a notification
    // email could craft a reply with a forged `From: someone-else`.
    // The reply posts as the TOKEN's `user_id` regardless of the
    // sender's `From`. This is the same property the existing v1
    // ingress already enforces for inbox-secret addresses (the secret
    // is the identity); we re-verify it for the reply namespace.
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    let raw = email_reply("not-bob@evil.example", &token, "still posts as bob");
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let posted = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(
        posted.user_id, fx.bob_id,
        "the reply token binds identity; the email From is advisory only",
    );
}

#[tokio::test]
async fn deleted_original_message_cascades_token_and_drops_as_unknown_address() {
    // The FK `reply_tokens.message_id REFERENCES messages(id) ON
    // DELETE CASCADE` means deleting the original reaps the token
    // automatically. After cascade, an incoming reply with that token
    // resolves to NotFound (the row is gone), which the poll loop
    // translates to `DropReason::AddressNoMatch`. This test pins the
    // contract so a future migration that drops the CASCADE (turning
    // it into a soft-reference) would surface as a test failure.
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(fx.original_id)
        .execute(&fx.state.chat)
        .await
        .unwrap();
    assert!(
        db::reply_tokens::resolve(&fx.state.chat, &token)
            .await
            .unwrap()
            .is_none(),
        "CASCADE must drop the reply-token row when the parent message is deleted",
    );
    let raw = email_reply("bob@example.com", &token, "race");
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason.as_str(), "address_no_match");
}

#[tokio::test]
async fn empty_body_after_strip_drops_with_parse_fail() {
    // A reply where the user wrote nothing new (just quoted the
    // original + signature) strips to empty and must drop, NOT post
    // an empty message row.
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    let body = "On Mon, May 25, 2026, alice wrote:\n> hello @bob";
    let raw = email_reply("bob@example.com", &token, body);
    let pre = message_count(&fx.state.chat, fx.room_id).await;
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason.as_str(), "parse_fail");
    assert_eq!(message_count(&fx.state.chat, fx.room_id).await, pre);
}

#[tokio::test]
async fn reply_in_cc_header_resolves_same_as_to() {
    // The resolver walks Delivered-To, X-Original-To, To, Cc in order.
    // A reply that lands with the reply-token in Cc only must still
    // post. Common when a user replied-all to a forwarded notification.
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;
    let raw = format!(
        "From: bob@example.com\r\n\
         To: someone-else@example.com\r\n\
         Cc: reply-{token}@{INGRESS_DOMAIN}\r\n\
         Subject: Re: [lets-chat] alice mentioned you in #ops\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <{}@spike.test>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         posted via Cc\r\n",
        uuid::Uuid::new_v4()
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "Cc-only reply must still resolve, got {outcome:?}",
    );
}

#[tokio::test]
async fn replay_after_consume_drops_with_address_no_match() {
    // First successful reply consumes the token. A second message
    // crafted against the same token (e.g., a forwarded notification
    // reaching a third party who tries to replay) drops with
    // `address_no_match` because the row no longer exists.
    let fx = setup().await;
    let token = mint(&fx.state.chat, &fx.bob_id, fx.original_id).await;

    let raw1 = email_reply("bob@example.com", &token, "first reply");
    assert!(matches!(
        process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw1).await,
        ProcessOutcome::Posted { .. }
    ));

    let raw2 = email_reply("not-bob@evil.example", &token, "replay attempt");
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw2).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(
        reason.as_str(),
        "address_no_match",
        "a one-shot token must not resolve after consume",
    );
}
