//! LC-668: auto thread titles. Covers the reply threshold, generation + storage,
//! idempotency, and the no-LLM no-op. Drives `routes::thread_title::run_thread_title`
//! directly with a mock LLM (the spawn in the reply handler is a thin wrapper).

use lets_chat::{db, routes::thread_title, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-threadtitle-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Setup {
    state: AppState,
    chat: SqlitePool,
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
        room_id,
        user_id: alice,
    }
}

fn mock(canned: &str) -> Arc<dyn lets_chat::llm::LlmClient> {
    Arc::new(lets_chat::llm::MockLlmClient {
        canned: canned.to_string(),
    })
}

/// Root message + `replies` thread replies; returns the root id.
async fn thread_with_replies(s: &Setup, replies: usize) -> i64 {
    let root = db::chat::insert_message(&s.chat, s.room_id, &s.user_id, "root question")
        .await
        .unwrap();
    for i in 0..replies {
        db::chat::insert_reply(&s.chat, s.room_id, &s.user_id, &format!("reply {i}"), root)
            .await
            .unwrap();
    }
    root
}

#[tokio::test]
async fn titles_a_thread_once_it_reaches_the_threshold() {
    let s = setup(Some(mock("\"Deploy the release.\""))).await;
    let root = thread_with_replies(&s, 3).await;

    let title = thread_title::run_thread_title(&s.state, s.room_id, root)
        .await
        .unwrap();
    // The model's quotes + trailing period are cleaned off.
    assert_eq!(title.as_deref(), Some("Deploy the release"));
    assert_eq!(
        db::chat::get_thread_title(&s.chat, root)
            .await
            .unwrap()
            .as_deref(),
        Some("Deploy the release"),
        "title is stored on the root"
    );

    // Idempotent: a re-run does not regenerate or change the stored title.
    let again = thread_title::run_thread_title(&s.state, s.room_id, root)
        .await
        .unwrap();
    assert_eq!(again, None, "already titled -> skipped");
}

#[tokio::test]
async fn does_not_title_a_thread_below_the_threshold() {
    let s = setup(Some(mock("Should not be used"))).await;
    let root = thread_with_replies(&s, 2).await; // below MIN_REPLIES

    let title = thread_title::run_thread_title(&s.state, s.room_id, root)
        .await
        .unwrap();
    assert_eq!(title, None);
    assert!(db::chat::get_thread_title(&s.chat, root)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn is_a_noop_without_an_llm() {
    let s = setup(None).await;
    let root = thread_with_replies(&s, 5).await;
    let title = thread_title::run_thread_title(&s.state, s.room_id, root)
        .await
        .unwrap();
    assert_eq!(title, None);
    assert!(db::chat::get_thread_title(&s.chat, root)
        .await
        .unwrap()
        .is_none());
}
