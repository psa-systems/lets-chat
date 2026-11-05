//! LC-1113: voice_calls groups the voice-event log into per-call spans.
use lets_chat::db::{voice_calls, voice_events};

use crate::common;

#[tokio::test]
async fn open_close_one_open_invariant_and_grouping() {
    let chat = common::chat_pool().await;
    let room = 7;

    // No open call to begin with.
    assert_eq!(voice_calls::open_id(&chat, room).await.unwrap(), None);

    // Open a call; events stamped with its id are grouped under it.
    let c1 = voice_calls::open(&chat, room).await.unwrap();
    assert_eq!(voice_calls::open_id(&chat, room).await.unwrap(), Some(c1));
    voice_events::log_event(&chat, room, "u1", "Alice", "connect", None, Some(c1))
        .await
        .unwrap();
    voice_events::log_event(
        &chat,
        room,
        "u1",
        "Alice",
        "error",
        Some("ice failed"),
        Some(c1),
    )
    .await
    .unwrap();

    // Opening again closes the stale call and starts a distinct one, so there is
    // always exactly one open call per room.
    let c2 = voice_calls::open(&chat, room).await.unwrap();
    assert_ne!(c1, c2);
    assert_eq!(
        voice_calls::open_id(&chat, room).await.unwrap(),
        Some(c2),
        "exactly one open call per room"
    );

    // close() ends the open call.
    voice_calls::close(&chat, room, Some(c2)).await.unwrap();
    assert_eq!(voice_calls::open_id(&chat, room).await.unwrap(), None);

    // The first call's timeline holds its two events; its summary counts them,
    // with the error flagged.
    let events = voice_calls::list_events(&chat, c1).await.unwrap();
    assert_eq!(events.len(), 2);
    let detail = voice_calls::get(&chat, c1)
        .await
        .unwrap()
        .expect("c1 exists");
    assert_eq!(detail.events, 2);
    assert_eq!(detail.errors, 1, "the error event is counted");
    assert_eq!(detail.participants, 1);

    // Both calls appear in the recent list, newest first (id tiebreaks a shared
    // second-granularity started_at).
    let recent = voice_calls::list_recent(&chat, 50).await.unwrap();
    assert!(recent.len() >= 2);
    assert_eq!(recent[0].id, c2, "newest call first");
}

/// LC-1122: `close` with an explicit `call_id` must scope the close to that
/// exact row, not just to `room_id`. A caller holding a stale id (resolved
/// before a new call opened) must not be able to end the newer call.
#[tokio::test]
async fn close_with_call_id_does_not_end_a_newer_call_for_the_same_room() {
    let chat = common::chat_pool().await;
    let room = 8;

    let c1 = voice_calls::open(&chat, room).await.unwrap();
    // A second open ends c1 and starts c2, same as a fresh join would.
    let c2 = voice_calls::open(&chat, room).await.unwrap();
    assert_ne!(c1, c2);

    // Closing with the stale c1 id must not touch the now-open c2.
    voice_calls::close(&chat, room, Some(c1)).await.unwrap();
    assert_eq!(
        voice_calls::open_id(&chat, room).await.unwrap(),
        Some(c2),
        "a close keyed by a stale call id must not end a newer call for the room"
    );

    // Closing with the current id ends it.
    voice_calls::close(&chat, room, Some(c2)).await.unwrap();
    assert_eq!(voice_calls::open_id(&chat, room).await.unwrap(), None);
}
