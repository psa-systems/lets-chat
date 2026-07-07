//! LC-62: scheduled dispatcher integration tests.
//!
//! Each test builds its own in-memory `AppState`, follows the
//! `push_dispatch` fixture pattern for the harness setup, and exercises
//! `scheduled::run_dispatch_tick` end-to-end (validation, atomic claim,
//! INSERT, post-commit `finalize_message_send`).

use lets_chat::db;
use lets_chat::push::{self, ReqwestPushClient};
use lets_chat::scheduled;
use lets_chat::state::AppState;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-sched-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

/// Build an AppState wired with empty in-memory pools and a no-op push
/// client. No VAPID, no mailer, no secret key (so `enforce_2fa_enrollment`
/// stays off via `secret_key.is_none()`).
async fn build_state() -> AppState {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let bg = lets_chat::bg::spawn(auth.clone());
    let push_client: Arc<dyn push::PushClient> = Arc::new(ReqwestPushClient::new(
        Arc::new(lets_chat::db::vapid::VapidKeypair {
            public_key_b64url: String::new(),
            private_key_bytes: vec![0u8; 32],
        }),
        "mailto:test@test".to_string(),
    ));
    AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: None,
        vapid: None,
        push_client,
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
    }
}

/// Create a user; promote the first one to admin (mirrors the live
/// auto-promote-first-registrant rule) so `backfill_general_membership`
/// has an admin to base the General enclave on.
async fn create_user_promote_first(state: &AppState, username: &str, first: bool) -> String {
    let id = db::auth::create_user(&state.auth, username, "hash")
        .await
        .unwrap();
    if first {
        sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
            .bind(&id)
            .execute(&state.auth)
            .await
            .unwrap();
    }
    id
}

/// Seed General enclave + room (id 1), with the two users as members.
async fn seed_general_with_users(state: &AppState) -> (String, String, i64) {
    let alice = create_user_promote_first(state, "alice", true).await;
    let bob = create_user_promote_first(state, "bob", false).await;
    db::enclave::backfill_general_membership(&state.auth, &state.chat)
        .await
        .unwrap();
    (alice, bob, 1)
}

#[tokio::test]
async fn past_scheduled_message_is_delivered_and_broadcast_fires() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    // Receiver registered before dispatch so the broadcast lands in the channel.
    let (_conn, mut rx, _) = state.hub.connect(&alice, "alice");

    let sched_id = db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "hello from the past",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.delivered, 1);
    assert_eq!(stats.dropped, 0);
    assert_eq!(stats.retries, 0);

    let row = db::scheduled::get_scheduled(&state.chat, sched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.delivered_at.is_some(), "scheduled row marked delivered");
    assert!(row.failed_reason.is_none(), "delivered, not dropped");

    let messages: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, user_id, body FROM messages WHERE room_id = ?")
            .bind(room_id)
            .fetch_all(&state.chat)
            .await
            .unwrap();
    assert_eq!(messages.len(), 1, "exactly one messages row inserted");
    assert_eq!(messages[0].1, alice);
    assert_eq!(messages[0].2, "hello from the past");

    let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("did not receive broadcast within 1s")
        .expect("hub channel closed");
    match evt {
        ChatEvent::NewMessage { message, .. } => {
            assert_eq!(message.body, "hello from the past");
            assert_eq!(message.user_id, alice);
        }
        other => panic!("expected NewMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn future_scheduled_message_is_not_delivered() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    let sched_id = db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "wait",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.delivered, 0);
    assert_eq!(stats.dropped, 0);

    let row = db::scheduled::get_scheduled(&state.chat, sched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.delivered_at.is_none(), "still pending");

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(&state.chat)
        .await
        .unwrap();
    assert_eq!(n, 0, "no messages row inserted");
}

#[tokio::test]
async fn user_kicked_out_of_enclave_gets_dropped_with_reason() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    let sched_id = db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "kicked",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Strip alice's enclave membership for the General enclave (id 1).
    sqlx::query("DELETE FROM enclave_members WHERE user_id = ?")
        .bind(&alice)
        .execute(&state.chat)
        .await
        .unwrap();
    // Also strip site-admin so the god-mode shortcut in is_room_accessible
    // does not bypass the membership check.
    sqlx::query("UPDATE users SET role = 'user' WHERE id = ?")
        .bind(&alice)
        .execute(&state.auth)
        .await
        .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.dropped, 1);
    assert_eq!(stats.delivered, 0);

    let row = db::scheduled::get_scheduled(&state.chat, sched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.delivered_at.is_some(), "marked at delivery attempt");
    assert_eq!(
        row.failed_reason.as_deref(),
        Some("author lost room access"),
        "reason surfaces for the management UI"
    );

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(&state.chat)
        .await
        .unwrap();
    assert_eq!(n, 0, "no messages row for the dropped delivery");
}

#[tokio::test]
async fn second_tick_after_delivery_is_a_noop() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "once only",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let s1 = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(s1.delivered, 1);
    let s2 = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(s2.delivered, 0, "delivered_at filter prevents re-delivery");
    assert_eq!(s2.dropped, 0);
    assert_eq!(s2.retries, 0);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(&state.chat)
        .await
        .unwrap();
    assert_eq!(n, 1, "exactly one messages row across two ticks");
}

#[tokio::test]
async fn row_in_backoff_cooldown_is_skipped_by_dispatch() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    let sched_id = db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "in cooldown",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    db::scheduled::bump_attempt(&state.chat, sched_id, 600)
        .await
        .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.delivered, 0);
    assert_eq!(stats.dropped, 0);
    assert_eq!(stats.retries, 0, "no rows even surfaced from select_due");

    let row = db::scheduled::get_scheduled(&state.chat, sched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.delivered_at.is_none(),
        "still pending after cooldown skip"
    );
    assert_eq!(row.attempt_count, 1, "bump_attempt incremented the count");
}

#[tokio::test]
async fn mention_target_renamed_between_schedule_and_delivery_drops_mention() {
    // Option B correctness check: mention resolution happens at delivery
    // against the current users table. A @bob token whose user was renamed
    // away from "bob" between schedule and delivery resolves to nobody;
    // no mentions row is inserted and the body keeps the literal "@bob"
    // text.
    let state = build_state().await;
    let (alice, bob, room_id) = seed_general_with_users(&state).await;

    db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "ping @bob",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Rename "bob" -> "robert" before the dispatcher tick. No helper exists
    // for username changes (today's product has no rename feature); raw
    // UPDATE simulates the eventual one.
    sqlx::query("UPDATE users SET username = 'robert' WHERE id = ?")
        .bind(&bob)
        .execute(&state.auth)
        .await
        .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.delivered, 1);

    let messages: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, body FROM messages WHERE room_id = ? ORDER BY id ASC")
            .bind(room_id)
            .fetch_all(&state.chat)
            .await
            .unwrap();
    assert_eq!(messages.len(), 1);
    let new_id = messages[0].0;
    assert_eq!(
        messages[0].1, "ping @bob",
        "body keeps the literal token; rename does not rewrite stored text"
    );

    let mention_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mentions WHERE message_id = ?")
            .bind(new_id)
            .fetch_one(&state.chat)
            .await
            .unwrap();
    assert_eq!(
        mention_count, 0,
        "no mentions row: parser found @bob, current users table has no @bob, resolver returned empty"
    );

    // Sanity: the renamed user still exists, just under a new username.
    let robert: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(&bob)
        .fetch_optional(&state.auth)
        .await
        .unwrap();
    assert_eq!(robert.as_deref(), Some("robert"));
}

// LC-485: a delivered recurring row enqueues its next occurrence atomically,
// as a fresh pending row carrying the same repeat kind and a future time.
#[tokio::test]
async fn recurring_message_reenqueues_next_occurrence() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    let sched_id = db::scheduled::insert_scheduled_with_repeat(
        &state.chat,
        room_id,
        &alice,
        "daily standup",
        "2000-01-01 09:00:00",
        None,
        None,
        None,
        "daily",
    )
    .await
    .unwrap();

    let stats = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats.delivered, 1);

    // Original row delivered.
    let orig = db::scheduled::get_scheduled(&state.chat, sched_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        orig.delivered_at.is_some(),
        "original recurring row delivered"
    );

    // Exactly one new pending row, future-dated, same repeat kind.
    let pending: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, scheduled_for, repeat FROM scheduled_messages \
         WHERE delivered_at IS NULL",
    )
    .fetch_all(&state.chat)
    .await
    .unwrap();
    assert_eq!(pending.len(), 1, "next occurrence enqueued");
    assert_ne!(pending[0].0, sched_id, "it is a new row");
    assert_eq!(pending[0].2, "daily");
    let future: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM scheduled_messages \
         WHERE delivered_at IS NULL AND scheduled_for > datetime('now')",
    )
    .fetch_one(&state.chat)
    .await
    .unwrap();
    assert_eq!(future, 1, "next occurrence is scheduled in the future");

    // The next tick does not re-deliver the just-delivered row.
    let stats2 = scheduled::run_dispatch_tick(&state).await.unwrap();
    assert_eq!(stats2.delivered, 0, "future occurrence not yet due");
}

// LC-485: a non-recurring row does NOT enqueue anything.
#[tokio::test]
async fn one_shot_message_does_not_reenqueue() {
    let state = build_state().await;
    let (alice, _bob, room_id) = seed_general_with_users(&state).await;

    db::scheduled::insert_scheduled(
        &state.chat,
        room_id,
        &alice,
        "one and done",
        "2000-01-01 09:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    scheduled::run_dispatch_tick(&state).await.unwrap();

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_messages WHERE delivered_at IS NULL")
            .fetch_one(&state.chat)
            .await
            .unwrap();
    assert_eq!(pending, 0, "one-shot leaves no pending row");
}
