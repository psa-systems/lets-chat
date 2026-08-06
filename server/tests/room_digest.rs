//! LC-665: scheduled per-room AI activity digest. Covers the opt-in gate, the
//! minimum-activity threshold, posting as the assistant bot, and the
//! once-per-interval dedupe. Drives `room_digest::run_digest_tick` directly with
//! a mock LLM (the timer loop in main.rs is a thin wrapper over it).

use lets_chat::{db, room_digest, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-digest-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Setup {
    state: AppState,
    chat: SqlitePool,
    auth: SqlitePool,
    room_id: i64,
    user_id: String,
}

async fn setup(llm: Option<Arc<dyn lets_chat::llm::LlmClient>>) -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    // LC-679: opt this AI-feature test into the runtime flag (default off).
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&alice)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &alice)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat: chat.clone(),
        settings,
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
        llm_client: llm,
        embedding_client: None,
    };
    Setup {
        state,
        chat,
        auth,
        room_id,
        user_id: alice,
    }
}

fn mock(canned: &str) -> Arc<dyn lets_chat::llm::LlmClient> {
    Arc::new(lets_chat::llm::MockLlmClient {
        canned: canned.to_string(),
    })
}

async fn seed_messages(s: &Setup, n: usize) {
    for i in 0..n {
        db::chat::insert_message(&s.chat, s.room_id, &s.user_id, &format!("message {i}"))
            .await
            .unwrap();
    }
}

async fn digest_count(s: &Setup) -> usize {
    db::chat::list_messages(&s.chat, s.room_id)
        .await
        .unwrap()
        .iter()
        .filter(|m| m.body.contains("Daily digest"))
        .count()
}

#[tokio::test]
async fn posts_a_digest_for_an_opted_in_busy_room_then_dedupes() {
    let s = setup(Some(mock("## Digest\n- We shipped the thing."))).await;
    db::chat::set_room_digest_enabled(&s.chat, s.room_id, true)
        .await
        .unwrap();
    seed_messages(&s, 6).await;

    let stats = room_digest::run_digest_tick(&s.state).await.unwrap();
    assert_eq!(stats.evaluated, 1);
    assert_eq!(stats.posted, 1);

    // Posted as the assistant bot, carrying the canned summary.
    let msgs = db::chat::list_messages(&s.chat, s.room_id).await.unwrap();
    let recap = msgs
        .iter()
        .find(|m| m.body.contains("Daily digest"))
        .expect("digest posted");
    assert!(
        recap.body.contains("We shipped the thing"),
        "{}",
        recap.body
    );
    let bot = db::auth::find_user_by_username(&s.auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot exists");
    assert_eq!(recap.user_id, bot.id, "authored by the assistant bot");

    // A second tick within the interval does not post again (dedupe via
    // digest_last_at), even with fresh activity.
    seed_messages(&s, 6).await;
    let stats2 = room_digest::run_digest_tick(&s.state).await.unwrap();
    assert_eq!(stats2.evaluated, 0, "room is no longer due");
    assert_eq!(stats2.posted, 0);
    assert_eq!(digest_count(&s).await, 1, "still exactly one digest");
}

#[tokio::test]
async fn skips_a_room_that_has_not_opted_in() {
    let s = setup(Some(mock("nope"))).await;
    seed_messages(&s, 10).await; // busy, but digest_enabled defaults off
    let stats = room_digest::run_digest_tick(&s.state).await.unwrap();
    assert_eq!(stats.evaluated, 0);
    assert_eq!(stats.posted, 0);
    assert_eq!(digest_count(&s).await, 0);
}

#[tokio::test]
async fn evaluates_but_does_not_post_for_a_quiet_room() {
    let s = setup(Some(mock("nope"))).await;
    db::chat::set_room_digest_enabled(&s.chat, s.room_id, true)
        .await
        .unwrap();
    seed_messages(&s, 2).await; // below the minimum

    let stats = room_digest::run_digest_tick(&s.state).await.unwrap();
    assert_eq!(stats.evaluated, 1, "the room was due and evaluated");
    assert_eq!(stats.posted, 0, "too little activity to bother posting");
    assert_eq!(digest_count(&s).await, 0);
    // It was still marked run, so it is no longer due next tick.
    assert!(
        db::chat::get_room_digest_last_at(&s.chat, s.room_id)
            .await
            .unwrap()
            .is_some(),
        "evaluation bumps the dedupe marker even when nothing is posted"
    );
}

#[tokio::test]
async fn is_a_noop_without_an_llm() {
    let s = setup(None).await;
    db::chat::set_room_digest_enabled(&s.chat, s.room_id, true)
        .await
        .unwrap();
    seed_messages(&s, 10).await;
    let stats = room_digest::run_digest_tick(&s.state).await.unwrap();
    assert_eq!(stats, room_digest::DigestStats::default());
    assert_eq!(digest_count(&s).await, 0);
}
