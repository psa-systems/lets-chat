//! LC-670: local AI moderation triage. Covers classify -> file-a-report-for-a-
//! human, the not-flagged and no-LLM no-ops, and idempotency. Drives
//! `routes::triage::run_triage` directly with a mock LLM (the spawn + env gate in
//! post_message is a thin wrapper over it). The report is filed as the assistant
//! bot into the same `/admin/reports` queue members' reports go to - triage
//! never deletes or hides a message.

use lets_chat::{db, routes::triage, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-triage-tests-{}", std::process::id()));
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

async fn post(s: &Setup, body: &str) -> i64 {
    db::chat::insert_message(&s.chat, s.room_id, &s.user_id, body)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_flagged_message_is_filed_for_human_review_as_the_bot() {
    let s = setup(Some(mock("HARASSMENT"))).await;
    let mid = post(&s, "you are all worthless").await;

    let flagged = triage::run_triage(&s.state, mid, s.room_id, "you are all worthless")
        .await
        .unwrap();
    assert_eq!(flagged, Some("harassment"));

    let open = db::reports::list_open(&s.chat).await.unwrap();
    assert_eq!(open.len(), 1, "one report filed for review");
    assert_eq!(open[0].message_id, mid);
    assert_eq!(open[0].category, "harassment");
    assert_eq!(open[0].status, "open");
    let bot = db::auth::find_user_by_username(&s.auth, "assistant")
        .await
        .unwrap()
        .expect("assistant bot exists");
    assert_eq!(open[0].reporter_id, bot.id, "filed by the AI triage bot");

    // The message itself is untouched - triage only flags, never deletes.
    assert!(
        db::chat::get_message(&s.chat, mid).await.unwrap().is_some(),
        "message is not auto-deleted"
    );

    // Idempotent: a re-run does not file a duplicate.
    triage::run_triage(&s.state, mid, s.room_id, "you are all worthless")
        .await
        .unwrap();
    assert_eq!(
        db::reports::count_open(&s.chat).await.unwrap(),
        1,
        "re-run does not duplicate the flag"
    );
}

#[tokio::test]
async fn a_clean_message_is_not_flagged() {
    let s = setup(Some(mock("NONE"))).await;
    let mid = post(&s, "sounds good, thanks!").await;
    let flagged = triage::run_triage(&s.state, mid, s.room_id, "sounds good, thanks!")
        .await
        .unwrap();
    assert_eq!(flagged, None);
    assert_eq!(db::reports::count_open(&s.chat).await.unwrap(), 0);
}

#[tokio::test]
async fn triage_is_a_noop_without_an_llm() {
    let s = setup(None).await;
    let mid = post(&s, "anything").await;
    let flagged = triage::run_triage(&s.state, mid, s.room_id, "anything")
        .await
        .unwrap();
    assert_eq!(flagged, None);
    assert_eq!(db::reports::count_open(&s.chat).await.unwrap(), 0);
}
