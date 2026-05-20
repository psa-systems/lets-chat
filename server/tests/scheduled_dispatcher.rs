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

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-sched-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
            include_str!("../migrations/auth/0007_notification_settings.sql"),
            include_str!("../migrations/auth/0008_two_factor.sql"),
            include_str!("../migrations/auth/0009_push_subscriptions.sql"),
            include_str!("../migrations/auth/0010_password_reset.sql"),
            include_str!("../migrations/auth/0011_email_verification.sql"),
            include_str!("../migrations/auth/0012_session_metadata.sql"),
            include_str!("../migrations/auth/0013_digest_columns.sql"),
            include_str!("../migrations/auth/0014_login_alerts.sql"),
            include_str!("../migrations/auth/0015_pending_registrations.sql"),
            include_str!("../migrations/auth/0016_sidebar_categories.sql"),
            include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql"),
            include_str!("../migrations/auth/0018_starred_rooms.sql"),
            include_str!("../migrations/auth/0019_api_tokens.sql"),
        ],
        "chat" => vec![
            include_str!("../migrations/chat/0001_create_tables.sql"),
            include_str!("../migrations/chat/0002_moderation.sql"),
            include_str!("../migrations/chat/0003_dms.sql"),
            include_str!("../migrations/chat/0004_message_editing.sql"),
            include_str!("../migrations/chat/0005_private_rooms.sql"),
            include_str!("../migrations/chat/0006_read_receipts.sql"),
            include_str!("../migrations/chat/0007_reactions.sql"),
            include_str!("../migrations/chat/0008_search.sql"),
            include_str!("../migrations/chat/0009_enclaves.sql"),
            include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
            include_str!("../migrations/chat/0011_threads.sql"),
            include_str!("../migrations/chat/0012_uploads.sql"),
            include_str!("../migrations/chat/0013_link_previews.sql"),
            include_str!("../migrations/chat/0014_mentions.sql"),
            include_str!("../migrations/chat/0015_room_notification_settings.sql"),
            include_str!("../migrations/chat/0016_pinned_messages.sql"),
            include_str!("../migrations/chat/0017_custom_emojis.sql"),
            include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
            include_str!("../migrations/chat/0019_bookmarks.sql"),
            include_str!("../migrations/chat/0020_quote_reply.sql"),
            include_str!("../migrations/chat/0021_enclave_invitations_enclave_idx.sql"),
            include_str!("../migrations/chat/0022_voice_messages.sql"),
            include_str!("../migrations/chat/0023_system_messages.sql"),
            include_str!("../migrations/chat/0024_voice_channel_flag.sql"),
            include_str!("../migrations/chat/0025_message_edits.sql"),
            include_str!("../migrations/chat/0026_room_categories.sql"),
            include_str!("../migrations/chat/0027_user_groups.sql"),
            include_str!("../migrations/chat/0028_room_role_overrides.sql"),
            include_str!("../migrations/chat/0029_room_posting_policy.sql"),
            include_str!("../migrations/chat/0030_room_docs_wiki.sql"),
            include_str!("../migrations/chat/0031_storage_quotas.sql"),
            include_str!("../migrations/chat/0032_anti_spam.sql"),
            include_str!("../migrations/chat/0033_scheduled_messages.sql"),
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_anti_spam.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
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
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
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
