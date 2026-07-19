//! LC-77-REPLY commit 1b: dispatcher gate tests for
//! `crate::email::notification::dispatch_mention_notification`.
//!
//! The tests exercise the gating order without requiring SMTP. Each test
//! constructs an `AppState` with `mailer: None` OR a mailer (none of the
//! production code paths actually need a live mailer to test gating).
//! The "Sent" branch is not exercised here - that path requires a real
//! SMTP server. The mailer-configured rate-limit test confirms the
//! counter ticks past the cap without depending on send success.

use std::sync::{Arc, OnceLock};

use lets_chat::email::notification::{
    dispatch_mention_notification, DispatchOutcome, NotificationKind, RATE_PER_MIN,
};
use lets_chat::{db, state::AppState, ws::hub::Hub};

mod common;

const SECRET: [u8; 32] = [29u8; 32];

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-notif-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    room_id: i64,
    room_name: String,
    sender_id: String,
    recipient_id: String,
    message_id: i64,
}

async fn setup(with_mailer: bool, verified: bool, opted_in: bool) -> Fixture {
    ensure_tempdir();
    let auth_pool = common::auth_pool().await;
    let chat_pool = common::chat_pool().await;
    let settings_pool = common::settings_pool().await;

    // Two users: sender and recipient. Recipient gets the email gates configured.
    let sender_id = db::auth::create_user(&auth_pool, "alice", "h")
        .await
        .unwrap();
    let recipient_id = db::auth::create_user(&auth_pool, "bob", "h").await.unwrap();

    db::auth::set_user_email(&auth_pool, &recipient_id, Some("bob@example.com"))
        .await
        .unwrap();
    if verified {
        db::auth::mark_email_verified(&auth_pool, &recipient_id, "bob@example.com")
            .await
            .unwrap();
    }
    if opted_in {
        db::auth::set_notify_email_activity_enabled(&auth_pool, &recipient_id, true)
            .await
            .unwrap();
    }

    // Room + a message authored by the sender that mentions the recipient.
    let room_id = db::chat::create_room(&chat_pool, "ops", None, "public", None, None)
        .await
        .unwrap();
    let message_id = db::chat::insert_message(&chat_pool, room_id, &sender_id, "hey @bob")
        .await
        .unwrap();

    let mailer = if with_mailer {
        // LC-363: a Mailer pointing at an unreachable host (we never actually
        // send). Built without mutating the process-global LETS_CHAT_SMTP_* env
        // vars, which raced across parallel test threads - a concurrent
        // remove_var could make a sibling test's from_env() return None.
        lets_chat::mail::Mailer::unreachable_for_tests()
    } else {
        None
    };

    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
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
        mailer,
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
        room_name: "ops".to_string(),
        sender_id,
        recipient_id,
        message_id,
    }
}

#[tokio::test]
async fn skipped_no_mailer_when_state_mailer_is_none() {
    let fx = setup(false, true, true).await;
    let outcome = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedNoMailer),
        "expected SkippedNoMailer; got {outcome:?}",
    );
    // No reply token should have been inserted: dispatcher short-circuits
    // before that step.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reply_tokens")
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn skipped_no_email_when_recipient_email_unset() {
    let fx = setup(true, false, true).await;
    // Override: clear the email row added by setup.
    sqlx::query("UPDATE users SET email = NULL, email_verified_at = NULL WHERE id = ?")
        .bind(&fx.recipient_id)
        .execute(&fx.state.auth)
        .await
        .unwrap();
    let outcome = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedNoEmail),
        "expected SkippedNoEmail; got {outcome:?}",
    );
}

#[tokio::test]
async fn skipped_unverified_when_email_verified_at_null() {
    let fx = setup(true, false, true).await;
    let outcome = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedUnverified),
        "expected SkippedUnverified; got {outcome:?}",
    );
}

#[tokio::test]
async fn skipped_opt_out_when_toggle_off() {
    let fx = setup(true, true, false).await;
    let outcome = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedOptOut),
        "expected SkippedOptOut; got {outcome:?}",
    );
}

#[tokio::test]
async fn skipped_no_recipient_when_user_id_unknown() {
    let fx = setup(true, true, true).await;
    let outcome = dispatch_mention_notification(
        &fx.state,
        "user-id-that-does-not-exist",
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedNoRecipient),
        "expected SkippedNoRecipient; got {outcome:?}",
    );
}

#[tokio::test]
async fn skipped_no_message_when_message_id_unknown() {
    let fx = setup(true, true, true).await;
    let outcome = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        99999999,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(outcome, DispatchOutcome::SkippedNoMessage),
        "expected SkippedNoMessage; got {outcome:?}",
    );
}

#[tokio::test]
async fn rate_limit_trips_after_cap() {
    let fx = setup(true, true, true).await;
    // Spend exactly RATE_PER_MIN allowances against the recipient; each call
    // should produce something OTHER than SkippedRateLimit (Sent or
    // SendFailed against the unreachable SMTP host both indicate the gate
    // passed). The N+1-th call must return SkippedRateLimit.
    let cap = RATE_PER_MIN as usize;
    let mut pre_limit_outcomes = Vec::with_capacity(cap);
    for _ in 0..cap {
        let outcome = dispatch_mention_notification(
            &fx.state,
            &fx.recipient_id,
            fx.message_id,
            NotificationKind::Mention,
            fx.room_id,
            &fx.room_name,
        )
        .await;
        assert!(
            !matches!(outcome, DispatchOutcome::SkippedRateLimit),
            "first {cap} attempts must not rate-limit; got {outcome:?}",
        );
        pre_limit_outcomes.push(outcome);
    }
    let next = dispatch_mention_notification(
        &fx.state,
        &fx.recipient_id,
        fx.message_id,
        NotificationKind::Mention,
        fx.room_id,
        &fx.room_name,
    )
    .await;
    assert!(
        matches!(next, DispatchOutcome::SkippedRateLimit),
        "{}-th attempt must trip the rate limit; got {next:?}",
        cap + 1,
    );

    // The cap-th and later attempts must have inserted reply tokens at
    // each non-rate-limited attempt that passed the gates (the SendFailed
    // result still produces the row before the send attempt).
    let token_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reply_tokens")
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    assert!(
        token_count > 0,
        "at least one reply token should have been minted before the rate limit tripped",
    );
    let _ = fx.sender_id; // unused in this test body but kept on Fixture for symmetry
}
