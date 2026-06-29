//! LC-494: stage control-plane unit tests - the ephemeral hub roster and the
//! per-room toggle. (The WS frames + per-viewer render are integration-level;
//! these cover the state machine + persistence.)

use lets_chat::db::chat;
use lets_chat::ws::hub::Hub;

mod common;

#[test]
fn stage_roster_promote_demote_hands_and_leave() {
    let hub = Hub::new();
    let room = 7;

    // Everyone joins as a listener.
    for u in ["host", "alice", "bob"] {
        hub.stage_join(room, u);
    }
    let r = hub.stage_roster(room).unwrap();
    assert_eq!(r.participants.len(), 3);
    assert!(r.speakers.is_empty());

    // alice raises a hand; bob does not.
    assert!(hub.stage_raise_hand(room, "alice"));
    assert!(hub.stage_roster(room).unwrap().hands.contains("alice"));

    // host promotes alice -> speaker, hand cleared.
    hub.stage_promote(room, "alice");
    let r = hub.stage_roster(room).unwrap();
    assert!(r.speakers.contains("alice"));
    assert!(!r.hands.contains("alice"));

    // A speaker cannot also have a raised hand.
    assert!(!hub.stage_raise_hand(room, "alice"));

    // Demote alice back to listener.
    hub.stage_demote(room, "alice");
    assert!(!hub.stage_roster(room).unwrap().speakers.contains("alice"));

    // Leaving removes from the roster; emptying the room drops the entry.
    for u in ["host", "alice", "bob"] {
        hub.stage_leave(room, u);
    }
    assert!(hub.stage_roster(room).is_none());
}

#[test]
fn stage_leave_all_reports_affected_rooms() {
    let hub = Hub::new();
    hub.stage_join(1, "u");
    hub.stage_join(2, "u");
    hub.stage_join(2, "other");
    let mut affected = hub.stage_leave_all("u");
    affected.sort();
    assert_eq!(affected, vec![1, 2]);
    // Room 1 emptied -> gone; room 2 still has `other`.
    assert!(hub.stage_roster(1).is_none());
    assert!(hub.stage_roster(2).unwrap().participants.contains("other"));
}

#[tokio::test]
async fn room_stage_toggle_roundtrips() {
    let pool = common::chat_pool().await;
    let room = chat::create_room(&pool, "stage", None, "public", None, None)
        .await
        .unwrap();
    assert!(!chat::get_room_stage_enabled(&pool, room).await.unwrap());
    assert_eq!(
        chat::set_room_stage_enabled(&pool, room, true)
            .await
            .unwrap(),
        1
    );
    assert!(chat::get_room_stage_enabled(&pool, room).await.unwrap());
}
