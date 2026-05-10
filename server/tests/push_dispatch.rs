use bytes::Bytes;
use lets_chat::db::notifications::MuteMode;
use lets_chat::db::push_subscriptions::PushSubscription;
use lets_chat::db::vapid::VapidKeypair;
use lets_chat::push::{self, MockPushClient, PushClient, PushError};
use lets_chat::ws::events::ChatEvent;
use lets_chat::{db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-push-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
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
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn fake_vapid() -> Arc<VapidKeypair> {
    Arc::new(VapidKeypair {
        public_key_b64url: "BPlaceholderpublickey".to_string(),
        private_key_bytes: vec![1u8; 32],
    })
}

struct Fixture {
    state: AppState,
    sender_id: String,
    recipient_id: String,
    room_id: i64,
    mock: Arc<MockPushClient>,
}

async fn fixture(client: Arc<dyn PushClient>, mock: Arc<MockPushClient>) -> Fixture {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;
    let sender_id = db::auth::create_user(&auth, "sender", "hash")
        .await
        .unwrap();
    let recipient_id = db::auth::create_user(&auth, "recipient", "hash")
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    // Seeded "general" room is id 1.
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: Some(fake_vapid()),
        push_client: client,
    };
    Fixture {
        state,
        sender_id,
        recipient_id,
        room_id: 1,
        mock,
    }
}

async fn enable_push(state: &AppState, user_id: &str) {
    db::auth::set_notification_prefs(&state.auth, user_id, true, false, true)
        .await
        .unwrap();
}

async fn add_sub(state: &AppState, user_id: &str, endpoint: &str) {
    db::push_subscriptions::insert_or_replace(
        &state.auth,
        user_id,
        endpoint,
        "p256dh-test",
        "auth-test",
        Some("ua"),
    )
    .await
    .unwrap();
}

fn mention_event(room_id: i64, recipient: &str) -> ChatEvent {
    ChatEvent::Mentioned {
        kind: "mention".into(),
        room_id,
        room_type: "public".into(),
        room_label: "#general".into(),
        message_id: 42,
        mentioned_user_id: recipient.into(),
        author_label: "alice".into(),
        snippet: "hi there".into(),
        target_path: format!("/room/{room_id}"),
    }
}

fn dm_event(room_id: i64, recipient: &str) -> ChatEvent {
    ChatEvent::Mentioned {
        kind: "dm".into(),
        room_id,
        room_type: "dm".into(),
        room_label: "alice".into(),
        message_id: 7,
        mentioned_user_id: recipient.into(),
        author_label: "alice".into(),
        snippet: "yo".into(),
        target_path: "/dm/sender".into(),
    }
}

/// Spawned tasks observe the row deletes / sends asynchronously. Yield a
/// few times so the runtime drains them before assertions.
async fn drain_spawns() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}

#[tokio::test]
async fn dispatch_sends_to_recipient_subscriptions() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let sent = f.mock.sent.lock().await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].endpoint, "https://e1.example/x");
    let json: serde_json::Value = serde_json::from_slice(&sent[0].payload).unwrap();
    assert_eq!(json["title"], "alice in #general");
    assert_eq!(json["body"], "hi there");
    assert_eq!(json["tag"], format!("lc-{}", f.room_id));
    assert_eq!(json["data"]["target_path"], format!("/room/{}", f.room_id));
    let _ = (f.sender_id,);
}

#[tokio::test]
async fn dispatch_skips_when_notify_push_disabled() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    // notify_push_enabled stays default (0).
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.mock.sent.lock().await.is_empty());
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_skips_when_vapid_unconfigured() {
    let mock = Arc::new(MockPushClient::default());
    let mut f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    f.state.vapid = None;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.mock.sent.lock().await.is_empty());
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_skips_when_room_muted_all() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    db::notifications::set_room_mute_mode(&f.state.chat, &f.recipient_id, f.room_id, MuteMode::All)
        .await
        .unwrap();

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.mock.sent.lock().await.is_empty());
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_fires_when_room_muted_except_mentions() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    db::notifications::set_room_mute_mode(
        &f.state.chat,
        &f.recipient_id,
        f.room_id,
        MuteMode::ExceptMentions,
    )
    .await
    .unwrap();

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert_eq!(f.mock.sent.lock().await.len(), 1);
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_skips_dm_kind_when_room_muted() {
    // Phase 17 removes the "DM bypass" that earlier let DM-kind events
    // skip the mute lookup. A DM-kind event whose target room is muted
    // (`MuteMode::All`) must now be dropped before any subscription send.
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    db::notifications::set_room_mute_mode(&f.state.chat, &f.recipient_id, f.room_id, MuteMode::All)
        .await
        .unwrap();

    let ev = dm_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.mock.sent.lock().await.is_empty());
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_fires_dm_kind_when_room_unmuted() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;

    let ev = dm_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let sent = f.mock.sent.lock().await;
    assert_eq!(sent.len(), 1);
    let json: serde_json::Value = serde_json::from_slice(&sent[0].payload).unwrap();
    assert_eq!(json["title"], "alice (DM)");
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_skips_when_no_subscriptions() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    // No subscriptions inserted.

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.mock.sent.lock().await.is_empty());
    let _ = f.sender_id;
}

#[tokio::test]
async fn dispatch_fan_out_one_per_subscription() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    add_sub(&f.state, &f.recipient_id, "https://e2.example/y").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let sent = f.mock.sent.lock().await;
    assert_eq!(sent.len(), 2);
    let mut endpoints: Vec<String> = sent.iter().map(|s| s.endpoint.clone()).collect();
    endpoints.sort();
    assert_eq!(
        endpoints,
        vec![
            "https://e1.example/x".to_string(),
            "https://e2.example/y".to_string(),
        ]
    );
    let _ = f.sender_id;
}

/// Test-only client that always returns `EndpointGone`. Drives the inline
/// 410 cleanup branch in `dispatch`.
struct GoneClient;

#[async_trait::async_trait]
impl PushClient for GoneClient {
    async fn send(&self, sub: &PushSubscription, _payload: Bytes) -> Result<(), PushError> {
        Err(PushError::EndpointGone(sub.endpoint.clone()))
    }
}

#[tokio::test]
async fn dispatch_410_deletes_subscription() {
    // We use a GoneClient as the production client; the mock is unused
    // here but `fixture` needs one so we pass an empty one.
    let mock = Arc::new(MockPushClient::default());
    let client: Arc<dyn PushClient> = Arc::new(GoneClient);
    let f = fixture(client, mock).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    add_sub(&f.state, &f.recipient_id, "https://e2.example/y").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let remaining = db::push_subscriptions::for_user(&f.state.auth, &f.recipient_id)
        .await
        .unwrap();
    assert!(remaining.is_empty(), "all subs should be deleted on 410");
    let _ = f.sender_id;
}

#[test]
fn payload_dm_kind_uses_dm_title_format() {
    let ev = dm_event(99, "rcpt");
    let bytes = push::payload::build(&ev).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["title"], "alice (DM)");
}

#[test]
fn payload_room_kind_uses_room_title_format() {
    let ev = mention_event(99, "rcpt");
    let bytes = push::payload::build(&ev).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["title"], "alice in #general");
}
