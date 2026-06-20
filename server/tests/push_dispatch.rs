use bytes::Bytes;
use lets_chat::db::notifications::MuteMode;
use lets_chat::db::push_subscriptions::PushSubscription;
use lets_chat::db::vapid::VapidKeypair;
use lets_chat::push::{
    self, ApnsClient, FcmClient, MockApnsClient, MockFcmClient, MockPushClient, PushClient,
    PushError,
};
use lets_chat::ws::events::ChatEvent;
use lets_chat::{db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

mod common;

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
    common::pool(name).await
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
    apns_mock: Arc<MockApnsClient>,
    fcm_mock: Arc<MockFcmClient>,
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
    let bg = lets_chat::bg::spawn(auth.clone());
    // LC-91: always wire mobile mocks so the multi-channel fan-out is
    // exercised. Web-Push-only tests register no APNs/FCM tokens, so these
    // record nothing and stay invisible to them.
    let apns_mock = Arc::new(MockApnsClient::default());
    let fcm_mock = Arc::new(MockFcmClient::default());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: Some(fake_vapid()),
        push_client: client,
        apns_client: Some(apns_mock.clone() as Arc<dyn ApnsClient>),
        fcm_client: Some(fcm_mock.clone() as Arc<dyn FcmClient>),
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
    };
    Fixture {
        state,
        sender_id,
        recipient_id,
        room_id: 1,
        mock,
        apns_mock,
        fcm_mock,
    }
}

async fn enable_push(state: &AppState, user_id: &str) {
    db::auth::set_notification_prefs(&state.auth, user_id, true, false, true, false)
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

async fn add_apns_sub(state: &AppState, user_id: &str, token: &str) {
    db::apns_subscriptions::insert_or_replace(
        &state.auth,
        user_id,
        token,
        Some("com.lc.app"),
        Some("ua"),
    )
    .await
    .unwrap();
}

async fn add_fcm_sub(state: &AppState, user_id: &str, token: &str) {
    db::fcm_subscriptions::insert_or_replace(&state.auth, user_id, token, Some("ua"))
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
async fn dispatch_fires_when_room_mute_none() {
    // Completes the none/except_mentions/all push triplet (LC-90): the
    // default unmuted mode delivers a mention push.
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    db::notifications::set_room_mute_mode(
        &f.state.chat,
        &f.recipient_id,
        f.room_id,
        MuteMode::None,
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

// ---- LC-91: mobile push (APNs / FCM) fan-out -----------------------------

/// Always-dead APNs token, mirrors `GoneClient` for the prune path.
struct GoneApns;
#[async_trait::async_trait]
impl ApnsClient for GoneApns {
    async fn send(
        &self,
        sub: &lets_chat::db::apns_subscriptions::ApnsSubscription,
        _p: Bytes,
    ) -> Result<(), PushError> {
        Err(PushError::EndpointGone(sub.device_token.clone()))
    }
}

/// Always-dead FCM token.
struct GoneFcm;
#[async_trait::async_trait]
impl FcmClient for GoneFcm {
    async fn send(
        &self,
        sub: &lets_chat::db::fcm_subscriptions::FcmSubscription,
        _p: Bytes,
    ) -> Result<(), PushError> {
        Err(PushError::EndpointGone(sub.registration_token.clone()))
    }
}

#[tokio::test]
async fn dispatch_fans_out_to_all_channels_in_parallel() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    add_sub(&f.state, &f.recipient_id, "https://e1.example/x").await;
    add_apns_sub(&f.state, &f.recipient_id, "apns-tok-1").await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-tok-1").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    // One delivery per channel, all carrying the same payload (AC: consistent
    // shape across channels).
    let web = f.mock.sent.lock().await;
    let apns = f.apns_mock.sent.lock().await;
    let fcm = f.fcm_mock.sent.lock().await;
    assert_eq!(web.len(), 1, "web push");
    assert_eq!(apns.len(), 1, "apns");
    assert_eq!(fcm.len(), 1, "fcm");
    assert_eq!(apns[0].token, "apns-tok-1");
    assert_eq!(fcm[0].token, "fcm-tok-1");
    assert_eq!(apns[0].payload, web[0].payload);
    assert_eq!(fcm[0].payload, web[0].payload);
}

#[tokio::test]
async fn apns_dead_token_is_pruned() {
    let mock = Arc::new(MockPushClient::default());
    let mut f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    f.state.apns_client = Some(Arc::new(GoneApns) as Arc<dyn ApnsClient>);
    enable_push(&f.state, &f.recipient_id).await;
    add_apns_sub(&f.state, &f.recipient_id, "apns-dead").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let remaining = db::apns_subscriptions::for_user(&f.state.auth, &f.recipient_id)
        .await
        .unwrap();
    assert!(
        remaining.is_empty(),
        "BadDeviceToken must prune the apns row"
    );
}

#[tokio::test]
async fn fcm_dead_token_is_pruned() {
    let mock = Arc::new(MockPushClient::default());
    let mut f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    f.state.fcm_client = Some(Arc::new(GoneFcm) as Arc<dyn FcmClient>);
    enable_push(&f.state, &f.recipient_id).await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-dead").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    let remaining = db::fcm_subscriptions::for_user(&f.state.auth, &f.recipient_id)
        .await
        .unwrap();
    assert!(
        remaining.is_empty(),
        "NOT_REGISTERED must prune the fcm row"
    );
}

#[tokio::test]
async fn mobile_channels_skipped_when_notify_disabled() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    // notify_push_enabled stays 0 (enable_push not called).
    add_apns_sub(&f.state, &f.recipient_id, "apns-tok").await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-tok").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.apns_mock.sent.lock().await.is_empty());
    assert!(f.fcm_mock.sent.lock().await.is_empty());
}

#[tokio::test]
async fn mobile_channels_skipped_when_room_muted_all() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    db::notifications::set_room_mute_mode(&f.state.chat, &f.recipient_id, f.room_id, MuteMode::All)
        .await
        .unwrap();
    add_apns_sub(&f.state, &f.recipient_id, "apns-tok").await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-tok").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.apns_mock.sent.lock().await.is_empty());
    assert!(f.fcm_mock.sent.lock().await.is_empty());
}

#[tokio::test]
async fn mobile_channels_skipped_during_dnd() {
    let mock = Arc::new(MockPushClient::default());
    let f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    enable_push(&f.state, &f.recipient_id).await;
    // LC-88 manual pause far into the future suppresses every channel.
    db::auth::set_dnd_pause(&f.state.auth, &f.recipient_id, Some("2099-01-01T00:00:00Z"))
        .await
        .unwrap();
    add_apns_sub(&f.state, &f.recipient_id, "apns-tok").await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-tok").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    assert!(f.apns_mock.sent.lock().await.is_empty());
    assert!(f.fcm_mock.sent.lock().await.is_empty());
}

#[tokio::test]
async fn unconfigured_mobile_channel_is_a_no_op_and_keeps_tokens() {
    let mock = Arc::new(MockPushClient::default());
    let mut f = fixture(mock.clone() as Arc<dyn PushClient>, mock.clone()).await;
    // Simulate production: no mobile sender wired.
    f.state.apns_client = None;
    f.state.fcm_client = None;
    enable_push(&f.state, &f.recipient_id).await;
    add_apns_sub(&f.state, &f.recipient_id, "apns-keep").await;
    add_fcm_sub(&f.state, &f.recipient_id, "fcm-keep").await;

    let ev = mention_event(f.room_id, &f.recipient_id);
    push::dispatch(&f.state, &f.recipient_id, &ev).await;
    drain_spawns().await;

    // Tokens survive (a missing sender must not look like a dead token).
    assert_eq!(
        db::apns_subscriptions::for_user(&f.state.auth, &f.recipient_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db::fcm_subscriptions::for_user(&f.state.auth, &f.recipient_id)
            .await
            .unwrap()
            .len(),
        1
    );
}
