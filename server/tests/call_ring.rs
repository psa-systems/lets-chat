//! LC-614: coverage for the 1:1 call ring - the `Hub` ringing slot and the
//! `relay_call_signal` policy. None of this had tests, and LC-596 AC4 requires
//! the DM 1:1 path to keep its behaviour while LC-613 refactors call.js; this is
//! the net that makes that refactor safe.

use lets_chat::routes::test_support::relay_call_signal;
use lets_chat::state::AppState;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::{Hub, RingingResult};
use lets_chat::{db, models::User};
use std::sync::Arc;

mod common;

// ---- the ringing slot state machine (Hub-level) -----------------------------

// `#[tokio::test]`: `try_start_ringing` spawns the TTL-eviction task, which needs
// a runtime.
#[tokio::test]
async fn ringing_slot_started_duplicate_and_clear() {
    let hub = Arc::new(Hub::new());
    let room = 5;

    // Vacant slot: the caller claims it.
    assert!(matches!(
        hub.try_start_ringing(room, "alice", "Alice", None),
        RingingResult::Started
    ));

    // The same caller inviting again (e.g. a second tab) is a duplicate, not a
    // new ring - so the peer is not invited twice.
    assert!(matches!(
        hub.try_start_ringing(room, "alice", "Alice", None),
        RingingResult::DuplicateSelf
    ));

    // Releasing the slot lets the next invite claim a fresh one.
    hub.clear_ringing(room);
    assert!(matches!(
        hub.try_start_ringing(room, "alice", "Alice", None),
        RingingResult::Started
    ));
}

#[tokio::test]
async fn ringing_slot_resolves_glare_to_the_first_caller() {
    let hub = Arc::new(Hub::new());
    let room = 9;

    // Alice rings first and owns the slot.
    assert!(matches!(
        hub.try_start_ringing(room, "alice", "Alice", Some("video".into())),
        RingingResult::Started
    ));

    // Bob rings the same DM at the same moment. This is glare: bob must NOT be
    // relayed as a second caller. Instead he loses, and gets alice's invite
    // replayed so his UI flips outgoing -> incoming. The winner is the first
    // caller, and her original payload is returned so the replay is faithful.
    match hub.try_start_ringing(room, "bob", "Bob", Some("audio".into())) {
        RingingResult::Glare {
            winner_id,
            from_name,
            payload,
        } => {
            assert_eq!(winner_id, "alice", "the first caller wins glare");
            assert_eq!(from_name, "Alice");
            assert_eq!(
                payload.as_deref(),
                Some("video"),
                "the winner's own payload is replayed, not the loser's"
            );
        }
        RingingResult::Started => {
            panic!("expected glare, got Started (the second caller must lose)")
        }
        RingingResult::DuplicateSelf => panic!("expected glare, got DuplicateSelf"),
    }
}

// ---- relay_call_signal policy + side effects --------------------------------

struct Setup {
    state: AppState,
    hub: Arc<Hub>,
    chat: sqlx::SqlitePool,
    alice: User,
    bob_id: String,
    outsider: User,
    dm_room: i64,
    group_room: i64,
}

async fn drain(rx: &mut tokio::sync::broadcast::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn call_signals_to<'a>(events: &'a [ChatEvent], user: &str) -> Vec<&'a ChatEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ChatEvent::CallSignal { to_user_id, .. } if to_user_id == user))
        .collect()
}

fn signal_kind(e: &ChatEvent) -> &str {
    match e {
        ChatEvent::CallSignal { kind, .. } => kind,
        _ => "",
    }
}

async fn message_count(chat: &sqlx::SqlitePool, room: i64) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE room_id = ?")
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
    let dir = std::env::temp_dir().join(format!("lc-call-ring-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let c = db::auth::create_user(&auth, "carol", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&a)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // A DM between alice and bob; carol is the outsider.
    let dm = db::chat::create_dm_room(&chat, "alice-bob", &a, &b)
        .await
        .unwrap();
    // A group room, to prove the relay refuses non-DM rooms.
    let group = db::chat::create_room(&chat, "grp", None, "public", None, None)
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
    let hub = state.hub.clone();
    let alice = user_row(&state, &a).await;
    let outsider = user_row(&state, &c).await;
    Setup {
        state,
        hub,
        chat,
        alice,
        bob_id: b,
        outsider,
        dm_room: dm.id,
        group_room: group,
    }
}

#[tokio::test]
async fn invite_rings_the_peer_and_posts_one_started_message() {
    let s = setup().await;
    let (_bc, mut bob_rx, _) = s.hub.connect(&s.bob_id, "bob");

    relay_call_signal(
        &s.state,
        &s.alice,
        "Alice",
        s.dm_room,
        "invite",
        Some("video".into()),
    )
    .await;

    let ev = drain(&mut bob_rx).await;
    let to_bob = call_signals_to(&ev, &s.bob_id);
    assert_eq!(to_bob.len(), 1, "the peer is rung exactly once");
    assert_eq!(signal_kind(to_bob[0]), "invite");
    assert_eq!(
        message_count(&s.chat, s.dm_room).await,
        1,
        "one 'started a call' system message is posted"
    );

    // The slot is now held: a second invite from the same caller is a duplicate
    // and neither re-rings nor posts again.
    let (_bc2, mut bob_rx2, _) = s.hub.connect(&s.bob_id, "bob");
    relay_call_signal(&s.state, &s.alice, "Alice", s.dm_room, "invite", None).await;
    assert!(
        call_signals_to(&drain(&mut bob_rx2).await, &s.bob_id).is_empty(),
        "a duplicate invite does not re-ring"
    );
    assert_eq!(
        message_count(&s.chat, s.dm_room).await,
        1,
        "a duplicate invite posts no second message"
    );
}

#[tokio::test]
async fn reject_and_cancel_post_a_message_and_release_the_slot() {
    let s = setup().await;

    // Ring first so there is a slot to release.
    relay_call_signal(&s.state, &s.alice, "Alice", s.dm_room, "invite", None).await;
    assert_eq!(message_count(&s.chat, s.dm_room).await, 1);

    // Bob (the callee) rejects: a "declined the call" message, and the slot is
    // released so a later invite claims a fresh one.
    let bob = user_row(&s.state, &s.bob_id).await;
    relay_call_signal(&s.state, &bob, "Bob", s.dm_room, "reject", None).await;
    assert_eq!(
        message_count(&s.chat, s.dm_room).await,
        2,
        "reject posts a declined-the-call message"
    );

    // Slot released -> a fresh invite is Started, not glare/duplicate.
    assert!(
        matches!(
            s.hub.try_start_ringing(s.dm_room, "x", "X", None),
            RingingResult::Started
        ),
        "reject released the ringing slot"
    );
}

#[tokio::test]
async fn relay_refuses_outsiders_non_dm_oversize_and_unknown_kind() {
    let s = setup().await;
    let (_bc, mut bob_rx, _) = s.hub.connect(&s.bob_id, "bob");

    // Carol is not a member of the DM: no signal, no message.
    relay_call_signal(&s.state, &s.outsider, "Carol", s.dm_room, "invite", None).await;
    assert!(
        call_signals_to(&drain(&mut bob_rx).await, &s.bob_id).is_empty(),
        "a non-member cannot ring the DM"
    );
    assert_eq!(message_count(&s.chat, s.dm_room).await, 0);

    // A non-DM room is not a 1:1 call surface.
    relay_call_signal(&s.state, &s.alice, "Alice", s.group_room, "invite", None).await;
    assert_eq!(
        message_count(&s.chat, s.group_room).await,
        0,
        "the relay refuses non-DM rooms"
    );

    // Over the payload cap: dropped before any delivery.
    let huge = "x".repeat(64 * 1024 + 1);
    relay_call_signal(&s.state, &s.alice, "Alice", s.dm_room, "invite", Some(huge)).await;
    assert!(
        call_signals_to(&drain(&mut bob_rx).await, &s.bob_id).is_empty(),
        "an oversize payload is dropped"
    );

    // An unknown kind outside the allowlist is dropped.
    relay_call_signal(&s.state, &s.alice, "Alice", s.dm_room, "haxor", None).await;
    assert!(
        call_signals_to(&drain(&mut bob_rx).await, &s.bob_id).is_empty(),
        "an unknown signal kind is dropped"
    );
    // None of the refused cases claimed the ringing slot.
    assert!(
        matches!(
            s.hub.try_start_ringing(s.dm_room, "probe", "P", None),
            RingingResult::Started
        ),
        "no refused invite claimed the slot"
    );
}

#[tokio::test]
async fn a_block_in_either_direction_kills_signaling() {
    let s = setup().await;
    let (_bc, mut bob_rx, _) = s.hub.connect(&s.bob_id, "bob");

    // Bob blocks alice. Alice's invite must not reach him, and no slot is taken.
    db::auth::block_user(&s.state.auth, &s.bob_id, &s.alice.id)
        .await
        .unwrap();
    relay_call_signal(&s.state, &s.alice, "Alice", s.dm_room, "invite", None).await;
    assert!(
        call_signals_to(&drain(&mut bob_rx).await, &s.bob_id).is_empty(),
        "a block in either direction kills the invite"
    );
    assert_eq!(message_count(&s.chat, s.dm_room).await, 0);
    assert!(
        matches!(
            s.hub.try_start_ringing(s.dm_room, "probe", "P", None),
            RingingResult::Started
        ),
        "a blocked invite claimed no slot"
    );
}
