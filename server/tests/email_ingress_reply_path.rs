//! LC-77-REPLY commit 2c: smoke test for the wired poll-loop -> reply
//! actor path.
//!
//! Confirms that a raw RFC 822 reply addressed to
//! `reply-<token>@<ingress-domain>` lands as a real-user post in the
//! original message's room (NOT as the email-inbox synthetic actor),
//! and the reply token is consumed.
//!
//! Threat-model coverage (banned user, blocked DM, expired token race,
//! quote-strip on hostile inputs) lands in
//! `email_ingress_reply_threat_model.rs` in commit 2d.

use std::sync::{Arc, OnceLock};

use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::{db, state::AppState, ws::hub::Hub};

mod common;

const SECRET: [u8; 32] = [21u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-reply-path-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    alice_id: String,
    room_id: i64,
    original_id: i64,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat, "Acme", None, &alice_id)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    // Alice mentioned herself in a public room earlier; the dispatcher
    // would have minted a reply token. We simulate the token by
    // inserting one directly.
    let original_id = db::chat::insert_message(&chat, room_id, &alice_id, "hello @alice")
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
    };
    Fixture {
        state,
        alice_id,
        room_id,
        original_id,
    }
}

fn email_with_to(addr: &str, body: &str) -> Vec<u8> {
    format!(
        "From: alice@example.com\r\n\
         To: {addr}\r\n\
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

#[tokio::test]
async fn reply_with_active_token_posts_as_real_user_and_consumes_token() {
    let fx = setup().await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(
        &fx.state.chat,
        &token,
        &fx.alice_id,
        fx.original_id,
        "2099-01-01 00:00:00",
    )
    .await
    .unwrap();

    let raw = email_with_to(
        &format!("reply-{token}@{INGRESS_DOMAIN}"),
        "thanks for the ping",
    );
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };

    let posted = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("freshly posted message row");
    assert_eq!(
        posted.user_id, fx.alice_id,
        "reply-by-email posts must be authored by the real user, not the synthetic email-inbox actor",
    );
    assert_eq!(
        posted.email_inbox_id, None,
        "reply path must NOT populate email_inbox_id (that field is reserved for the synthetic-actor path)",
    );
    assert_eq!(
        posted.webhook_id, None,
        "reply path must NOT populate webhook_id",
    );
    assert_eq!(posted.room_id, fx.room_id);
    assert_eq!(posted.body, "thanks for the ping");

    // Token was consumed.
    let resolved = db::reply_tokens::resolve(&fx.state.chat, &token)
        .await
        .unwrap();
    assert!(
        resolved.is_none(),
        "a successful reply post must consume the token (one-shot)",
    );
}

#[tokio::test]
async fn expired_token_drops_without_post() {
    let fx = setup().await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(
        &fx.state.chat,
        &token,
        &fx.alice_id,
        fx.original_id,
        "2000-01-01 00:00:00",
    )
    .await
    .unwrap();

    let pre_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(fx.room_id)
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();

    let raw = email_with_to(&format!("reply-{token}@{INGRESS_DOMAIN}"), "too late");
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped for expired token, got {outcome:?}");
    };
    assert_eq!(reason.as_str(), "reply_expired");

    let post_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(fx.room_id)
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    assert_eq!(
        pre_count, post_count,
        "an expired reply must not insert a message row",
    );
}

#[tokio::test]
async fn reply_strips_quoted_original_and_signature() {
    let fx = setup().await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(
        &fx.state.chat,
        &token,
        &fx.alice_id,
        fx.original_id,
        "2099-01-01 00:00:00",
    )
    .await
    .unwrap();

    let body =
        "great point\n\nOn Mon, May 25, 2026, alice wrote:\n> hello @alice\n\n-- \nalice via phone";
    let raw = email_with_to(&format!("reply-{token}@{INGRESS_DOMAIN}"), body);
    let outcome = process_polled_message(&fx.state, &SECRET, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };

    let posted = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(
        posted.body, "great point",
        "the reply body must be stripped of the quoted original and the signature",
    );
}
