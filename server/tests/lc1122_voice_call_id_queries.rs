//! LC-1122: `record_voice_event` used to call `db::voice_calls::open_id`
//! internally on every voice lifecycle event, including the join that just
//! opened the call it was about to re-look-up. This test pins the fixed query
//! shape the fix produces: a huddle-starting join costs zero `voice_calls`
//! lookups (the id `open` already returned is threaded straight through), and
//! every other event (diag, leave) costs exactly the one lookup its call site
//! cannot avoid, never a second one hidden inside `record_voice_event` itself.
//!
//! Uses a `tracing::Subscriber` installed as the global default (LC-782's
//! pattern, reused rather than shared since this test - like LC-782's - installs
//! a process-wide subscriber and so keeps its own test binary, per the note in
//! `tests/it/main.rs`): `sqlx-sqlite` runs every query on a dedicated
//! per-connection worker thread, so a thread-local subscriber never sees it.
use lets_chat::routes::test_support::{handle_voice_diag, handle_voice_join, handle_voice_leave};
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, models::User};
use std::sync::Arc;

#[path = "common/mod.rs"]
mod common;

static EVENTS: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Vec<String>>>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));

struct GlobalCapturingSubscriber;

impl tracing::Subscriber for GlobalCapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct FieldsToString(String);
        impl tracing::field::Visit for FieldsToString {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{}={:?}", field.name(), value));
            }
        }
        let mut visitor = FieldsToString(String::new());
        event.record(&mut visitor);
        EVENTS.lock().unwrap().push(visitor.0);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn install_global_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(GlobalCapturingSubscriber).ok();
    });
}

/// Count of `SELECT id FROM voice_calls ...` (i.e. `voice_calls::open_id`)
/// statements logged since the last call to this helper.
fn drain_open_id_queries() -> usize {
    EVENTS
        .lock()
        .unwrap()
        .drain(..)
        .filter(|e| e.contains("SELECT id FROM voice_calls"))
        .count()
}

async fn user_row(state: &AppState, id: &str) -> User {
    db::auth::find_user_by_id(&state.auth, id)
        .await
        .unwrap()
        .expect("user exists")
        .into()
}

#[tokio::test(flavor = "current_thread")]
async fn one_join_one_diag_pair_and_one_leave_cost_at_most_three_open_id_lookups() {
    install_global_subscriber();

    let dir = std::env::temp_dir().join(format!("lc-1122-call-id-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    // `backfill_general_membership` is a no-op with no admin user yet (needs an
    // owner to assign), so make alice one; matches `tests/it/huddle.rs`'s setup.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&a)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room = 1; // General, both members after the backfill.

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
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
        llm_client: None,
        embedding_client: None,
    };
    let hub = state.hub.clone();
    let alice = user_row(&state, &a).await;

    // Drain anything logged by setup (user creation, backfill) before timing
    // the scenario itself.
    drain_open_id_queries();

    // Join: the room is empty, so this starts the huddle. `handle_voice_join`
    // must thread the id `voice_calls::open` returns straight into
    // `record_voice_event` with no further `voice_calls` lookup.
    let (conn, _rx, _) = hub.connect(&a, "alice");
    handle_voice_join(&state, &alice, "alice", conn, room).await;
    assert_eq!(
        drain_open_id_queries(),
        0,
        "a huddle-starting join must cost zero voice_calls lookups"
    );
    assert!(
        hub.is_in_voice_room(conn, room),
        "alice must be registered as a participant of the room after joining"
    );

    // A mute/unmute pair has no just-opened id to reuse (same shape as the
    // diag call site exercised here, since VoiceMute itself is only reachable
    // through a live WS frame loop with no test seam): each occurrence costs
    // its own single lookup, done once at the call site rather than hidden
    // inside `record_voice_event`.
    handle_voice_diag(&state, &alice, "alice", conn, room, "diag one".into(), None).await;
    assert_eq!(
        drain_open_id_queries(),
        1,
        "a diag/mute-shaped event with no cached call id costs exactly one lookup"
    );
    handle_voice_diag(&state, &alice, "alice", conn, room, "diag two".into(), None).await;
    assert_eq!(
        drain_open_id_queries(),
        1,
        "a second such event costs exactly one lookup too, not zero or two"
    );

    // Leave: resolves the call id once and reuses it for the departure event;
    // alice is the only participant, so this also closes the call, but that
    // close is keyed by room_id and costs no extra voice_calls lookup.
    handle_voice_leave(&state, conn, "left");
    // handle_voice_leave's DB work runs on a spawned task; wait for the event
    // to land before counting.
    for _ in 0..100 {
        if !EVENTS
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.contains("INSERT INTO voice_events"))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        } else {
            break;
        }
    }
    assert_eq!(
        drain_open_id_queries(),
        1,
        "leave resolves the call id once and must not look it up again for close"
    );
}
