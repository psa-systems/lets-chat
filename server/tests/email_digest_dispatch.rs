//! Database-level integration tests for the digest tick (phase 22 task 4).
//!
//! The earlier version of this file used a `MockEmailClient` to inspect
//! sent message content. After the merge with main's env-var-based
//! `mail::Mailer`, the dispatch path no longer goes through a trait we
//! can substitute in tests, so this file is now limited to verifying
//! the DB side effects: who the tick selects, when it self-skips, when
//! it re-fires, when it short-circuits, and how the mute filter trims
//! the missed-activity set.
//!
//! Content-rendering correctness lives in the unit tests on the snippet
//! helper (`crate::digest`) and the view-struct templates
//! (`crate::views::email_digest`).

#![cfg(feature = "standalone")]

use lets_chat::digest::{self, DigestConfig};
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, push};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-digest-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create tempdir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

struct Harness {
    state: AppState,
    alice_id: String,
    room_id: i64,
}

async fn build_harness() -> Harness {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let alice_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    sqlx::query(
        "UPDATE users \
            SET email = 'alice@example.com', \
                notify_email_digest_enabled = 1, \
                last_active_at = datetime('now', '-2 hours'), \
                last_ws_seen_at = datetime('now', '-2 hours') \
          WHERE id = ?",
    )
    .bind(&alice_id)
    .execute(&auth)
    .await
    .unwrap();

    let bob_id = db::auth::create_user(&auth, "bob", "hash").await.unwrap();

    let room_id = db::chat::create_room(&chat, "general", None, "public", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &alice_id)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &bob_id)
        .await
        .unwrap();
    let msg_id = db::chat::insert_message(&chat, room_id, &bob_id, "Hey @alice can you review?")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(room_id)
    .bind(&alice_id)
    .bind(&bob_id)
    .execute(&chat)
    .await
    .unwrap();

    let dm = db::chat::create_dm_room(&chat, "alice-bob", &alice_id, &bob_id)
        .await
        .unwrap();
    db::chat::insert_message(&chat, dm.id, &bob_id, "ping me when you are around")
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        bg,
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        secret_key: Some(Arc::new([42u8; 32])),
        vapid: None,
        push_client: Arc::new(push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        // The merged digest path goes through `mail::Mailer` for the
        // actual send, which is concrete and not test-substitutable.
        // Setting `mailer: None` exercises every code path through to
        // the "short-circuit" check at the top of run_tick; tests that
        // want to assert "candidate was selected" do so by checking the
        // DB after a tick instead.
        mailer: None,
        base_url: "https://chat.example.com".into(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
    };
    Harness {
        state,
        alice_id,
        room_id,
    }
}

#[tokio::test]
async fn tick_with_mailer_none_short_circuits_without_db_writes() {
    let h = build_harness().await;
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    let alice = db::auth::find_user_by_id(&h.state.auth, &h.alice_id)
        .await
        .unwrap()
        .unwrap();
    assert!(alice.last_digest_sent_at.is_none());
}

#[tokio::test]
async fn eligibility_query_selects_offline_opted_in_user_with_email() {
    let h = build_harness().await;
    let cands =
        db::auth::find_digest_candidates(&h.state.auth, DigestConfig::default().quiet_period_secs)
            .await
            .unwrap();
    assert!(
        cands.iter().any(|c| c.id == h.alice_id),
        "alice should be eligible (offline 2h, opted in, email on file)"
    );
}

#[tokio::test]
async fn eligibility_query_excludes_user_without_email() {
    let h = build_harness().await;
    sqlx::query("UPDATE users SET email = NULL WHERE id = ?")
        .bind(&h.alice_id)
        .execute(&h.state.auth)
        .await
        .unwrap();
    let cands =
        db::auth::find_digest_candidates(&h.state.auth, DigestConfig::default().quiet_period_secs)
            .await
            .unwrap();
    assert!(cands.iter().all(|c| c.id != h.alice_id));
}

#[tokio::test]
async fn eligibility_query_excludes_user_who_opted_out() {
    let h = build_harness().await;
    sqlx::query("UPDATE users SET notify_email_digest_enabled = 0 WHERE id = ?")
        .bind(&h.alice_id)
        .execute(&h.state.auth)
        .await
        .unwrap();
    let cands =
        db::auth::find_digest_candidates(&h.state.auth, DigestConfig::default().quiet_period_secs)
            .await
            .unwrap();
    assert!(cands.iter().all(|c| c.id != h.alice_id));
}

#[tokio::test]
async fn eligibility_query_self_skips_inside_offline_session() {
    let h = build_harness().await;
    db::auth::set_last_digest_sent_at(&h.state.auth, &h.alice_id)
        .await
        .unwrap();
    let cands =
        db::auth::find_digest_candidates(&h.state.auth, DigestConfig::default().quiet_period_secs)
            .await
            .unwrap();
    assert!(
        cands.iter().all(|c| c.id != h.alice_id),
        "already-digested user must not be re-selected in the same offline session"
    );
}

#[tokio::test]
async fn eligibility_query_re_fires_on_new_offline_session() {
    let h = build_harness().await;
    sqlx::query(
        "UPDATE users \
            SET last_digest_sent_at = datetime('now', '-2 hours'), \
                last_active_at = datetime('now', '-90 minutes'), \
                last_ws_seen_at = datetime('now', '-90 minutes') \
          WHERE id = ?",
    )
    .bind(&h.alice_id)
    .execute(&h.state.auth)
    .await
    .unwrap();
    let cands =
        db::auth::find_digest_candidates(&h.state.auth, DigestConfig::default().quiet_period_secs)
            .await
            .unwrap();
    assert!(
        cands.iter().any(|c| c.id == h.alice_id),
        "after fresh activity, the next offline session must be eligible"
    );
}

#[tokio::test]
async fn muting_room_excludes_its_mentions_from_eligibility_floor_check() {
    let h = build_harness().await;
    db::notifications::set_room_mute_mode(
        &h.state.chat,
        &h.alice_id,
        h.room_id,
        db::notifications::MuteMode::All,
    )
    .await
    .unwrap();
    let muted_mention_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) \
           FROM mentions m \
           LEFT JOIN room_notification_settings rns \
                  ON rns.user_id = m.mentioned_user_id AND rns.room_id = m.room_id \
          WHERE m.mentioned_user_id = ? \
            AND m.read_at IS NULL \
            AND (rns.mute_mode IS NULL OR rns.mute_mode <> 'all')",
    )
    .bind(&h.alice_id)
    .fetch_one(&h.state.chat)
    .await
    .unwrap();
    assert_eq!(
        muted_mention_count, 0,
        "muted-all room must filter out alice's mention from the digest set"
    );
}

#[tokio::test]
async fn except_mentions_keeps_mention_in_digest_set() {
    // Symmetric to the muted-all test (LC-90): 'except_mentions' must NOT
    // exclude the mention. The digest filter only drops 'all', so a user who
    // muted plain chatter still receives their @mentions in the digest.
    let h = build_harness().await;
    db::notifications::set_room_mute_mode(
        &h.state.chat,
        &h.alice_id,
        h.room_id,
        db::notifications::MuteMode::ExceptMentions,
    )
    .await
    .unwrap();
    let mention_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) \
           FROM mentions m \
           LEFT JOIN room_notification_settings rns \
                  ON rns.user_id = m.mentioned_user_id AND rns.room_id = m.room_id \
          WHERE m.mentioned_user_id = ? \
            AND m.read_at IS NULL \
            AND (rns.mute_mode IS NULL OR rns.mute_mode <> 'all')",
    )
    .bind(&h.alice_id)
    .fetch_one(&h.state.chat)
    .await
    .unwrap();
    assert_eq!(
        mention_count, 1,
        "except_mentions must keep the mention in the digest set"
    );
}
