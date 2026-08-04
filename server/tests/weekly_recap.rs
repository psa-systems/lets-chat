//! LC-671: personal weekly recap DM. Covers candidacy (active + due), the
//! quiet-week skip, DM delivery as the assistant bot, and the once-per-week
//! dedupe. Drives `weekly_recap::run_weekly_recap_tick` directly with a mock LLM
//! (the 6h timer + env gate in main.rs is a thin wrapper).

use lets_chat::{db, state::AppState, weekly_recap, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-recap-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Setup {
    state: AppState,
    chat: SqlitePool,
    auth: SqlitePool,
    room_id: i64,
}

async fn setup(llm: Option<Arc<dyn lets_chat::llm::LlmClient>>) -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, None)
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
    }
}

fn mock(canned: &str) -> Arc<dyn lets_chat::llm::LlmClient> {
    Arc::new(lets_chat::llm::MockLlmClient {
        canned: canned.to_string(),
    })
}

/// Create an active (recently seen) real user.
async fn active_user(s: &Setup, name: &str) -> String {
    let id = db::auth::create_user(&s.auth, name, "h").await.unwrap();
    sqlx::query("UPDATE users SET last_active_at = datetime('now') WHERE id = ?")
        .bind(&id)
        .execute(&s.auth)
        .await
        .unwrap();
    id
}

async fn dm_from_bot(s: &Setup, user_id: &str) -> Option<String> {
    let bot = db::auth::find_user_by_username(&s.auth, "assistant")
        .await
        .unwrap()?;
    let room = db::chat::find_dm_room(&s.chat, &bot.id, user_id)
        .await
        .unwrap()?;
    let msgs = db::chat::list_messages(&s.chat, room.id).await.unwrap();
    msgs.into_iter()
        .find(|m| m.user_id == bot.id)
        .map(|m| m.body)
}

#[tokio::test]
async fn dms_an_active_user_their_recap_then_dedupes() {
    let s = setup(Some(mock(
        "Nice work this week - you were busy and got some love!",
    )))
    .await;
    let alice = active_user(&s, "alice").await;
    // Give alice a week worth of activity: a message and a kudos received.
    db::chat::insert_message(&s.chat, s.room_id, &alice, "hello team")
        .await
        .unwrap();
    db::kudos::record(
        &s.chat,
        "bob",
        &alice,
        s.room_id,
        None,
        Some("great help"),
        None,
    )
    .await
    .unwrap();

    let stats = weekly_recap::run_weekly_recap_tick(&s.state).await.unwrap();
    assert_eq!(stats.evaluated, 1);
    assert_eq!(stats.sent, 1);

    let body = dm_from_bot(&s, &alice)
        .await
        .expect("a recap DM from the bot");
    assert!(body.contains("Nice work this week"), "recap body: {body}");

    // Once-per-week dedupe: a second tick finds nobody due (marker bumped).
    let stats2 = weekly_recap::run_weekly_recap_tick(&s.state).await.unwrap();
    assert_eq!(stats2.evaluated, 0, "no longer due this week");
    assert_eq!(stats2.sent, 0);
}

#[tokio::test]
async fn skips_a_user_with_a_quiet_week() {
    let s = setup(Some(mock("should not be sent"))).await;
    let bob = active_user(&s, "bob").await; // active login, but no messages / kudos

    let stats = weekly_recap::run_weekly_recap_tick(&s.state).await.unwrap();
    assert_eq!(stats.evaluated, 1, "bob is active, so he is evaluated");
    assert_eq!(stats.sent, 0, "but a quiet week gets no DM");
    assert!(dm_from_bot(&s, &bob).await.is_none());
    // Still marked so he is not re-evaluated until next week.
    let stats2 = weekly_recap::run_weekly_recap_tick(&s.state).await.unwrap();
    assert_eq!(stats2.evaluated, 0);
}

#[tokio::test]
async fn is_a_noop_without_an_llm() {
    let s = setup(None).await;
    let alice = active_user(&s, "alice").await;
    db::chat::insert_message(&s.chat, s.room_id, &alice, "hi")
        .await
        .unwrap();
    let stats = weekly_recap::run_weekly_recap_tick(&s.state).await.unwrap();
    assert_eq!(stats, weekly_recap::RecapStats::default());
    assert!(dm_from_bot(&s, &alice).await.is_none());
}
