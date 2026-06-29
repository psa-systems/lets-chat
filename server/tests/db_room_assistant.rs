//! LC-492: room assistant toggle + FTS retrieval helper.

use lets_chat::db::chat;
mod common;

#[tokio::test]
async fn assistant_enabled_toggle_roundtrips() {
    let pool = common::chat_pool().await;
    let room = chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    assert!(!chat::get_room_assistant_enabled(&pool, room).await.unwrap());
    assert_eq!(
        chat::set_room_assistant_enabled(&pool, room, true)
            .await
            .unwrap(),
        1
    );
    assert!(chat::get_room_assistant_enabled(&pool, room).await.unwrap());
    chat::set_room_assistant_enabled(&pool, room, false)
        .await
        .unwrap();
    assert!(!chat::get_room_assistant_enabled(&pool, room).await.unwrap());
}

#[tokio::test]
async fn fts_room_context_matches_in_room_only() {
    let pool = common::chat_pool().await;
    let room = chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    let other = chat::create_room(&pool, "other", None, "public", None, None)
        .await
        .unwrap();
    chat::insert_message(&pool, room, "u-1", "the deploy runbook lives in the wiki")
        .await
        .unwrap();
    chat::insert_message(&pool, room, "u-2", "unrelated chatter about lunch")
        .await
        .unwrap();
    chat::insert_message(&pool, other, "u-3", "deploy notes in another room")
        .await
        .unwrap();

    let fts = chat::sanitize_fts_query("deploy").expect("query");
    let hits = chat::fts_room_context(&pool, room, &fts, 12).await.unwrap();
    assert_eq!(hits.len(), 1, "only the in-room deploy message matches");
    assert_eq!(hits[0].0, "u-1");
    assert!(hits[0].1.contains("deploy runbook"));
}
