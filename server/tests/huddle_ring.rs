//! LC-611: a huddle now rings the room's members.
//!
//! `VoiceJoined` is broadcast to the room topic, so it only reaches sockets that
//! already have the room open - the people who least need telling. These tests
//! cover the addressed `HuddleStarted` fan-out that replaces "notice the huddle
//! bar" with "be told".

use lets_chat::db;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use std::sync::Arc;

mod common;

/// The real row, so the test exercises what the handler passes.
async fn user(state: &lets_chat::state::AppState, id: &str) -> lets_chat::models::User {
    db::auth::find_user_by_id(&state.auth, id)
        .await
        .unwrap()
        .expect("user exists")
        .into()
}

/// Drain whatever is queued on a receiver without blocking.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn rings_for<'a>(events: &'a [ChatEvent], user: &str) -> Vec<&'a ChatEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ChatEvent::HuddleStarted { to_user_id, .. } if to_user_id == user))
        .collect()
}

/// The ring reaches a member who does not have the room open, which is the
/// whole point - and it reaches them once, not once per later joiner.
#[tokio::test]
async fn huddle_start_rings_members_once() {
    let (state, room, users) = setup().await;
    let (alice, bob, carol) = (&users[0], &users[1], &users[2]);

    // Bob and Carol are connected but subscribed to nothing: they are elsewhere
    // in the app. A room-topic broadcast would never reach them.
    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");
    let (_cc, mut carol_rx, _) = state.hub.connect(carol, "carol");
    let (alice_conn, mut alice_rx, _) = state.hub.connect(alice, "alice");

    state.hub.voice_join(alice_conn, room);
    lets_chat::huddle_ring::ring_members(&state, &user(&state, alice).await, "alice", room, true)
        .await;

    let bob_events = drain(&mut bob_rx);
    assert_eq!(
        rings_for(&bob_events, bob).len(),
        1,
        "a member elsewhere in the app is rung exactly once"
    );
    assert_eq!(rings_for(&drain(&mut carol_rx), carol).len(), 1);
    assert!(
        rings_for(&drain(&mut alice_rx), alice).is_empty(),
        "the starter is not rung by their own huddle"
    );

    // Carol joins the huddle that is already running. Nobody is rung again -
    // the ring marks a huddle starting, not every arrival.
    let (carol_conn, _rx, _) = state.hub.connect(carol, "carol");
    let mut bob_rx2 = state.hub.connect(bob, "bob").1;
    // The real caller passes `started = voice_room_users(room).is_empty()`
    // captured before joining; the mesh is non-empty now, so this is false.
    assert!(
        !state.hub.voice_room_users(room).is_empty(),
        "precondition: the huddle is already running"
    );
    state.hub.voice_join(carol_conn, room);
    lets_chat::huddle_ring::ring_members(&state, &user(&state, carol).await, "carol", room, false)
        .await;
    assert!(
        rings_for(&drain(&mut bob_rx2), bob).is_empty(),
        "a second joiner must not re-ring the room"
    );
}

/// A muted room does not ring. `ExceptMentions` also suppresses: a huddle
/// starting is not a mention, so "interrupt me only for mentions" must mean it.
#[tokio::test]
async fn muted_members_are_not_rung() {
    let (state, room, users) = setup().await;
    let (alice, bob, carol) = (&users[0], &users[1], &users[2]);

    db::notifications::set_room_mute_mode(&state.chat, bob, room, db::notifications::MuteMode::All)
        .await
        .unwrap();
    db::notifications::set_room_mute_mode(
        &state.chat,
        carol,
        room,
        db::notifications::MuteMode::ExceptMentions,
    )
    .await
    .unwrap();

    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");
    let (_cc, mut carol_rx, _) = state.hub.connect(carol, "carol");
    let (alice_conn, _rx, _) = state.hub.connect(alice, "alice");

    state.hub.voice_join(alice_conn, room);
    lets_chat::huddle_ring::ring_members(&state, &user(&state, alice).await, "alice", room, true)
        .await;

    assert!(
        rings_for(&drain(&mut bob_rx), bob).is_empty(),
        "mute_mode=all must not ring"
    );
    assert!(
        rings_for(&drain(&mut carol_rx), carol).is_empty(),
        "mute_mode=except_mentions must not ring: a huddle is not a mention"
    );
}

/// An enclave voice channel is a place people go to deliberately. The first
/// person through the door has not summoned anyone, so it must not ring.
#[tokio::test]
async fn joining_a_voice_channel_does_not_ring() {
    let (state, _room, users) = setup().await;
    let (alice, bob) = (&users[0], &users[1]);

    let voice_room = db::chat::create_room(&state.chat, "voicechan", None, "public", None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE rooms SET is_voice = 1 WHERE id = ?")
        .bind(voice_room)
        .execute(&state.chat)
        .await
        .unwrap();
    for u in [alice, bob] {
        db::chat::add_room_member(&state.chat, voice_room, u)
            .await
            .unwrap();
    }

    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");
    let (alice_conn, _rx, _) = state.hub.connect(alice, "alice");
    state.hub.voice_join(alice_conn, voice_room);
    lets_chat::huddle_ring::ring_members(
        &state,
        &user(&state, alice).await,
        "alice",
        voice_room,
        true,
    )
    .await;

    assert!(
        rings_for(&drain(&mut bob_rx), bob).is_empty(),
        "entering a persistent voice channel must not ring the room"
    );
}

/// Build state with one group room containing three members.
async fn setup() -> (lets_chat::state::AppState, i64, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("lc-huddle-ring-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let mut users = Vec::new();
    for name in ["alice", "bob", "carol"] {
        users.push(db::auth::create_user(&auth, name, "h").await.unwrap());
    }
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&users[0])
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let room = db::chat::create_room(&chat, "huddleroom", None, "public", None, None)
        .await
        .unwrap();
    for u in &users {
        db::chat::add_room_member(&chat, room, u).await.unwrap();
    }

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = lets_chat::state::AppState {
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
    (state, room, users)
}

fn presence_for<'a>(events: &'a [ChatEvent], user: &str) -> Vec<&'a ChatEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ChatEvent::HuddlePresence { to_user_id, .. } if to_user_id == user))
        .collect()
}

fn presence_count(e: &ChatEvent) -> i64 {
    match e {
        ChatEvent::HuddlePresence { count, .. } => *count,
        _ => -1,
    }
}

/// LC-612: the sidebar in-call indicator reaches a member who does NOT have the
/// room open (that is the whole point), carries the post-change count, and
/// fires on every join/leave rather than just the first.
#[tokio::test]
async fn presence_fans_out_to_members_with_the_current_count() {
    let (state, room, users) = setup().await;
    let (alice, bob, carol) = (&users[0], &users[1], &users[2]);

    // Bob is elsewhere in the app (subscribed to no room). A room-topic
    // broadcast would miss him.
    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");

    // Alice joins: bob's sidebar learns the room has 1.
    let (alice_conn, _rx, _) = state.hub.connect(alice, "alice");
    state.hub.voice_join(alice_conn, room);
    lets_chat::huddle_ring::broadcast_presence(&state, room).await;
    let ev = drain(&mut bob_rx);
    let p = presence_for(&ev, bob);
    assert_eq!(p.len(), 1, "a member elsewhere is told about the huddle");
    assert_eq!(presence_count(p[0]), 1, "count is 1 after the first join");

    // Carol joins: the count rises to 2, and bob is told again (not just once).
    let (carol_conn, _rx, _) = state.hub.connect(carol, "carol");
    state.hub.voice_join(carol_conn, room);
    lets_chat::huddle_ring::broadcast_presence(&state, room).await;
    let ev = drain(&mut bob_rx);
    let p = presence_for(&ev, bob);
    assert_eq!(p.len(), 1, "a later join re-notifies");
    assert_eq!(presence_count(p[0]), 2, "count rises with the second join");

    // Alice leaves: the count falls to 1 (broadcast_presence reads the roster
    // after the leave, as the real handler does).
    state.hub.voice_leave(alice_conn);
    lets_chat::huddle_ring::broadcast_presence(&state, room).await;
    let ev = drain(&mut bob_rx);
    let p = presence_for(&ev, bob);
    assert_eq!(presence_count(p[0]), 1, "count falls when someone leaves");

    // Carol leaves: empty. A 0 count tears the indicator down.
    state.hub.voice_leave(carol_conn);
    lets_chat::huddle_ring::broadcast_presence(&state, room).await;
    let ev = drain(&mut bob_rx);
    let p = presence_for(&ev, bob);
    assert_eq!(
        presence_count(p[0]),
        0,
        "an empty huddle clears the indicator"
    );
}

/// LC-612: unlike the ring, the indicator ignores mute - it is passive presence,
/// not an interruption, so a member who muted the room still sees the dot.
#[tokio::test]
async fn presence_ignores_mute() {
    let (state, room, users) = setup().await;
    let (alice, bob) = (&users[0], &users[1]);

    db::notifications::set_room_mute_mode(&state.chat, bob, room, db::notifications::MuteMode::All)
        .await
        .unwrap();

    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");
    let (alice_conn, _rx, _) = state.hub.connect(alice, "alice");
    state.hub.voice_join(alice_conn, room);
    lets_chat::huddle_ring::broadcast_presence(&state, room).await;

    assert_eq!(
        presence_for(&drain(&mut bob_rx), bob).len(),
        1,
        "a muted member still gets the passive in-call indicator"
    );
}

/// LC-612: a persistent voice channel does not drive the huddle indicator (its
/// membership is enclave-scoped, and it is not the ad-hoc huddle this is for).
#[tokio::test]
async fn presence_skips_voice_channels() {
    let (state, _room, users) = setup().await;
    let (alice, bob) = (&users[0], &users[1]);

    let voice_room = db::chat::create_room(&state.chat, "voicechan", None, "public", None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE rooms SET is_voice = 1 WHERE id = ?")
        .bind(voice_room)
        .execute(&state.chat)
        .await
        .unwrap();
    for u in [alice, bob] {
        db::chat::add_room_member(&state.chat, voice_room, u)
            .await
            .unwrap();
    }

    let (_bc, mut bob_rx, _) = state.hub.connect(bob, "bob");
    let (alice_conn, _rx, _) = state.hub.connect(alice, "alice");
    state.hub.voice_join(alice_conn, voice_room);
    lets_chat::huddle_ring::broadcast_presence(&state, voice_room).await;

    assert!(
        presence_for(&drain(&mut bob_rx), bob).is_empty(),
        "a voice channel must not drive the huddle indicator"
    );
}
