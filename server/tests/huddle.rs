//! LC-614: coverage for the huddle / voice-channel WS handlers
//! (`handle_voice_join` / `handle_voice_leave`). These drive the mesh roster
//! that LC-613 will refactor and LC-615's SFU handover builds on, and they had
//! no direct tests - only indirect exercise through the transcription and ring
//! suites.

use lets_chat::routes::test_support::{handle_voice_join, handle_voice_leave};
use lets_chat::state::AppState;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::{ConnId, Hub};
use lets_chat::{db, models::User};
use std::sync::Arc;

mod common;

fn drain(rx: &mut tokio::sync::broadcast::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn has_joined(events: &[ChatEvent], room: i64, user: &str) -> bool {
    events.iter().any(
        |e| matches!(e, ChatEvent::VoiceJoined { room_id, user_id, .. } if *room_id == room && user_id == user),
    )
}

fn has_left(events: &[ChatEvent], room: i64, user: &str) -> bool {
    events.iter().any(
        |e| matches!(e, ChatEvent::VoiceLeft { room_id, user_id } if *room_id == room && user_id == user),
    )
}

fn has_roster_for(events: &[ChatEvent], user: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, ChatEvent::VoiceRoster { to_user_id, .. } if to_user_id == user))
}

struct Setup {
    state: AppState,
    hub: Arc<Hub>,
    alice: User,
    alice_id: String,
    bob_id: String,
    room: i64,
}

async fn user_row(state: &AppState, id: &str) -> User {
    db::auth::find_user_by_id(&state.auth, id)
        .await
        .unwrap()
        .expect("user exists")
        .into()
}

async fn setup() -> Setup {
    let dir = std::env::temp_dir().join(format!("lc-huddle-hdl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&a)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    // General (room 1) is a group room both are members of after the backfill.
    let room = 1;

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
    Setup {
        state,
        hub,
        alice,
        alice_id: a,
        bob_id: b,
        room,
    }
}

#[tokio::test]
async fn join_registers_rosters_the_joiner_and_announces() {
    let s = setup().await;

    // Bob is already in the huddle (registered directly) and subscribed so he
    // hears the announcement.
    let (bob_conn, mut bob_rx, _) = s.hub.connect(&s.bob_id, "bob");
    s.hub.subscribe(bob_conn, s.room);
    s.hub.voice_join(bob_conn, s.room);

    // Alice joins through the handler.
    let (alice_conn, mut alice_rx, _) = s.hub.connect(&s.alice_id, "alice");
    s.hub.subscribe(alice_conn, s.room);
    handle_voice_join(&s.state, &s.alice, "alice", alice_conn, s.room).await;

    // She is now in the roster.
    assert!(
        s.hub
            .voice_room_users(s.room)
            .iter()
            .any(|u| u == &s.alice_id),
        "the joiner is registered in the voice roster"
    );
    // She gets the roster of who was already there.
    assert!(
        has_roster_for(&drain(&mut alice_rx), &s.alice_id),
        "the joiner receives a VoiceRoster"
    );
    // Bob, already present, is told alice joined.
    assert!(
        has_joined(&drain(&mut bob_rx), s.room, &s.alice_id),
        "existing participants are announced the joiner"
    );
}

#[tokio::test]
async fn join_is_refused_for_a_room_the_user_cannot_access() {
    let s = setup().await;

    // A private room with no enclave. Bob is a plain member (not admin), so he
    // has no access - the access gate, not membership in the call, is what stops
    // him. (Alice would pass here: she is a site admin.)
    let private = db::chat::create_room(&s.state.chat, "secret", None, "private", None, None)
        .await
        .unwrap();
    let bob = user_row(&s.state, &s.bob_id).await;

    let (conn, _rx, _) = s.hub.connect(&s.bob_id, "bob");
    handle_voice_join(&s.state, &bob, "bob", conn, private).await;

    assert!(
        s.hub.voice_room_users(private).is_empty(),
        "a user with no access to the room does not join its huddle"
    );
}

#[tokio::test]
async fn one_user_on_two_connections_counts_once() {
    let s = setup().await;

    let (c1, _r1, _) = s.hub.connect(&s.alice_id, "alice");
    let (c2, _r2, _) = s.hub.connect(&s.alice_id, "alice");
    handle_voice_join(&s.state, &s.alice, "alice", c1, s.room).await;
    handle_voice_join(&s.state, &s.alice, "alice", c2, s.room).await;

    let users = s.hub.voice_room_users(s.room);
    assert_eq!(
        users.iter().filter(|u| **u == s.alice_id).count(),
        1,
        "voice_room_users dedupes by user, not by connection"
    );
}

#[tokio::test]
async fn leave_announces_and_removes_from_the_roster() {
    let s = setup().await;

    // Bob stays; he should hear alice leave.
    let (bob_conn, mut bob_rx, _) = s.hub.connect(&s.bob_id, "bob");
    s.hub.subscribe(bob_conn, s.room);
    s.hub.voice_join(bob_conn, s.room);

    let (alice_conn, _rx, _) = s.hub.connect(&s.alice_id, "alice");
    handle_voice_join(&s.state, &s.alice, "alice", alice_conn, s.room).await;
    let _ = drain(&mut bob_rx); // clear the join announcement

    handle_voice_leave(&s.state, alice_conn);

    assert!(
        !s.hub
            .voice_room_users(s.room)
            .iter()
            .any(|u| u == &s.alice_id),
        "the leaver is removed from the roster"
    );
    assert!(
        has_left(&drain(&mut bob_rx), s.room, &s.alice_id),
        "remaining participants are told the leaver left"
    );
}

#[tokio::test]
async fn leave_on_an_unregistered_connection_is_a_noop() {
    let s = setup().await;
    // A connection that never joined any voice room: leaving must not panic or
    // emit, and must not disturb an unrelated participant.
    let (bob_conn, _rx, _) = s.hub.connect(&s.bob_id, "bob");
    s.hub.voice_join(bob_conn, s.room);

    let never_joined: ConnId = 999_999;
    handle_voice_leave(&s.state, never_joined);

    assert_eq!(
        s.hub.voice_room_users(s.room).len(),
        1,
        "an unrelated leave does not disturb the roster"
    );
}
