//! LC-853: coverage for the huddle remote-control consent relay - sharer
//! routing, the single-controller gate, the workspace switch, and the
//! share-stop auto-revoke. Driven through the `test_support` seam like
//! `call_ring.rs`, so the policy is tested without a WS framing harness.

use lets_chat::routes::test_support::{
    end_control_on_share_stop, relay_control_signal, REMOTE_CONTROL_ENABLED_KEY,
};
use lets_chat::state::AppState;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, models::User};
use std::sync::Arc;

mod common;

struct Setup {
    state: AppState,
    hub: Arc<Hub>,
    chat: sqlx::SqlitePool,
    alice: User,
    bob: User,
    carol: User,
    outsider: User,
    room: i64,
}

async fn drain(rx: &mut tokio::sync::broadcast::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn control_kinds_to(events: &[ChatEvent], user: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::RemoteControlSignal {
                to_user_id, kind, ..
            } if to_user_id == user => Some(kind.clone()),
            _ => None,
        })
        .collect()
}

async fn open_session_count(chat: &sqlx::SqlitePool, room: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM remote_control_sessions WHERE room_id = ? AND ended_at IS NULL",
    )
    .bind(room)
    .fetch_one(chat)
    .await
    .unwrap()
}

async fn user_row(state: &AppState, id: &str) -> User {
    db::auth::find_user_by_id(&state.auth, id)
        .await
        .unwrap()
        .expect("user exists")
        .into()
}

async fn setup() -> Setup {
    let dir = std::env::temp_dir().join(format!("lc-huddle-rc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let c = db::auth::create_user(&auth, "carol", "h").await.unwrap();
    let d = db::auth::create_user(&auth, "dave", "h").await.unwrap();
    // LC-183's gate requires both sides email-verified (standalone build).
    for id in [&a, &b, &c, &d] {
        sqlx::query("UPDATE users SET email_verified_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(&auth)
            .await
            .unwrap();
    }
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // A group room: the huddle path. (DM rooms keep the LC-183 relay.)
    let room = db::chat::create_room(&chat, "grp", None, "public", None, None)
        .await
        .unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
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
        llm_client: None,
        embedding_client: None,
    };
    // The switch defaults OFF; most tests flip it on explicitly.
    let hub = state.hub.clone();
    let alice = user_row(&state, &a).await;
    let bob = user_row(&state, &b).await;
    let carol = user_row(&state, &c).await;
    let outsider = user_row(&state, &d).await;
    Setup {
        state,
        hub,
        chat,
        alice,
        bob,
        carol,
        outsider,
        room,
    }
}

async fn enable(s: &Setup) {
    db::settings::set_setting(&s.state.settings, REMOTE_CONTROL_ENABLED_KEY, "true")
        .await
        .unwrap();
}

/// Join a user to the huddle (one connection) and mark them sharing if asked.
fn join(s: &Setup, user: &User, sharing: bool) -> tokio::sync::broadcast::Receiver<ChatEvent> {
    let (conn, rx, _) = s.hub.connect(&user.id, &user.username);
    s.hub.voice_join(conn, s.room);
    if sharing {
        s.hub.set_voice_screen(s.room, &user.id, true);
    }
    rx
}

#[tokio::test]
async fn request_routes_to_the_single_sharer_and_grant_opens_one_session() {
    let s = setup().await;
    enable(&s).await;
    let mut alice_rx = join(&s, &s.alice, true); // alice shares
    let mut bob_rx = join(&s, &s.bob, false);
    let mut carol_rx = join(&s, &s.carol, false);

    // Bob asks: the request lands on the sharer, nobody else.
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut alice_rx).await, &s.alice.id),
        vec!["request"],
        "the sharer is prompted"
    );

    // Carol asks while bob's request is pending: refused as busy, and the
    // sharer is NOT prompted a second time (no queue, no shaming).
    relay_control_signal(&s.state, &s.carol, "Carol", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut carol_rx).await, &s.carol.id),
        vec!["busy"],
        "a second requester is refused while one is pending"
    );
    assert!(
        control_kinds_to(&drain(&mut alice_rx).await, &s.alice.id).is_empty(),
        "the sharer sees no second prompt"
    );

    // Alice grants: bob gets the grant and exactly one audit row opens with
    // the right roles.
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "grant").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["grant"]
    );
    assert_eq!(open_session_count(&s.chat, s.room).await, 1);
    let (controller, sharer): (String, String) = sqlx::query_as(
        "SELECT controller_id, sharer_id FROM remote_control_sessions
         WHERE room_id = ? AND ended_at IS NULL",
    )
    .bind(s.room)
    .fetch_one(&s.chat)
    .await
    .unwrap();
    assert_eq!(controller, s.bob.id);
    assert_eq!(sharer, s.alice.id);

    // While bob controls, carol is still refused as busy.
    relay_control_signal(&s.state, &s.carol, "Carol", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut carol_rx).await, &s.carol.id),
        vec!["busy"]
    );

    // Alice revokes: bob is told, the row closes.
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "revoke").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["revoke"]
    );
    assert_eq!(open_session_count(&s.chat, s.room).await, 0);
}

#[tokio::test]
async fn switch_off_refuses_requests_and_deny_grant_rules_hold() {
    let s = setup().await;
    let mut alice_rx = join(&s, &s.alice, true);
    let mut bob_rx = join(&s, &s.bob, false);

    // Flag never set: the request is answered `unavailable`, the sharer never
    // hears about it.
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["unavailable"]
    );
    assert!(control_kinds_to(&drain(&mut alice_rx).await, &s.alice.id).is_empty());

    enable(&s).await;

    // A grant with no pending request answers nobody and opens nothing.
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "grant").await;
    assert!(control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id).is_empty());
    assert_eq!(open_session_count(&s.chat, s.room).await, 0);

    // Deny clears the pending request: a grant AFTER a deny answers nobody.
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "deny").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["deny"]
    );
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "grant").await;
    assert!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id).is_empty(),
        "a grant after the deny has no pending requester to answer"
    );
    assert_eq!(open_session_count(&s.chat, s.room).await, 0);

    // Only the sharer may answer: bob requests, CAROL (not sharing) cannot
    // grant bob control of alice's screen.
    let _carol_rx = join(&s, &s.carol, false);
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    relay_control_signal(&s.state, &s.carol, "Carol", s.room, "grant").await;
    assert_eq!(open_session_count(&s.chat, s.room).await, 0);
    assert!(!control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id).contains(&"grant".to_string()));
}

#[tokio::test]
async fn ambiguous_or_absent_sharer_and_outsiders_are_refused() {
    let s = setup().await;
    enable(&s).await;
    let mut bob_rx = join(&s, &s.bob, false);

    // Nobody sharing: unavailable.
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["unavailable"]
    );

    // Two sharers: ambiguous target, unavailable.
    let _alice_rx = join(&s, &s.alice, true);
    let _carol_rx = join(&s, &s.carol, true);
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["unavailable"]
    );

    // A user who is NOT in the huddle is dropped silently - even the request
    // echo would leak that the surface exists.
    let (_, mut outsider_rx, _) = s.hub.connect(&s.outsider.id, &s.outsider.username);
    relay_control_signal(&s.state, &s.outsider, "Dave", s.room, "request").await;
    assert!(control_kinds_to(&drain(&mut outsider_rx).await, &s.outsider.id).is_empty());
}

#[tokio::test]
async fn share_stop_auto_revokes_and_tells_the_controller() {
    let s = setup().await;
    enable(&s).await;
    let mut alice_rx = join(&s, &s.alice, true);
    let mut bob_rx = join(&s, &s.bob, false);

    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "grant").await;
    drain(&mut alice_rx).await;
    drain(&mut bob_rx).await;
    assert_eq!(open_session_count(&s.chat, s.room).await, 1);

    // Alice's share ends (the VoiceScreen{sharing:false} path): the session
    // closes with the share and the controller is told to stop.
    s.hub.set_voice_screen(s.room, &s.alice.id, false);
    end_control_on_share_stop(&s.state, s.room, &s.alice.id).await;
    assert_eq!(open_session_count(&s.chat, s.room).await, 0);
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["revoke"]
    );
    let reason: String = sqlx::query_scalar(
        "SELECT end_reason FROM remote_control_sessions
         WHERE room_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(s.room)
    .fetch_one(&s.chat)
    .await
    .unwrap();
    assert_eq!(reason, "share_ended");
}

// ---- LC-855 phase 3: audit events, per-room disable, room-wide label --------

async fn audit_kinds(chat: &sqlx::SqlitePool, room: i64) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT kind FROM remote_control_events WHERE room_id = ? ORDER BY id",
    )
    .bind(room)
    .fetch_all(chat)
    .await
    .unwrap()
}

fn control_labels(events: &[ChatEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::VoiceControlChanged {
                controller_name,
                active,
                ..
            } => Some((controller_name.clone(), *active)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn every_consent_event_is_audited_and_the_room_is_labelled() {
    let s = setup().await;
    enable(&s).await;
    let _alice_rx = join(&s, &s.alice, true);
    let _bob_rx = join(&s, &s.bob, false);
    // A third participant observes the room-wide control label broadcast.
    let mut carol_rx = join(&s, &s.carol, false);

    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "grant").await;
    relay_control_signal(&s.state, &s.alice, "Alice", s.room, "revoke").await;

    // The audit captured all three events (request+grant+revoke), in order -
    // not only the session open/close the LC-186 table already had.
    assert_eq!(
        audit_kinds(&s.chat, s.room).await,
        vec!["request", "grant", "revoke"]
    );

    // Carol (an uninvolved participant) saw the room-wide label go active with
    // the controller's resolved name, then clear.
    let labels = control_labels(&drain(&mut carol_rx).await);
    assert!(
        labels.iter().any(|(name, active)| *active && name == "bob"),
        "grant broadcasts an active label naming the controller: {labels:?}"
    );
    assert!(
        labels.iter().any(|(_, active)| !*active),
        "revoke broadcasts a clear"
    );
}

#[tokio::test]
async fn per_room_disable_blocks_requests_under_the_workspace_switch() {
    let s = setup().await;
    enable(&s).await; // workspace ON
    db::chat::set_room_remote_control_disabled(&s.chat, s.room, true)
        .await
        .unwrap();
    let mut alice_rx = join(&s, &s.alice, true);
    let mut bob_rx = join(&s, &s.bob, false);

    // The room opted out: a request is refused even though the workspace is on.
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut bob_rx).await, &s.bob.id),
        vec!["unavailable"]
    );
    assert!(control_kinds_to(&drain(&mut alice_rx).await, &s.alice.id).is_empty());
    assert!(audit_kinds(&s.chat, s.room).await.is_empty());

    // Re-enabling the room lets it through again.
    db::chat::set_room_remote_control_disabled(&s.chat, s.room, false)
        .await
        .unwrap();
    relay_control_signal(&s.state, &s.bob, "Bob", s.room, "request").await;
    assert_eq!(
        control_kinds_to(&drain(&mut alice_rx).await, &s.alice.id),
        vec!["request"]
    );
}
