//! LC-546: thread_muters db layer - mute/unmute/is_muted/muters and FK cascade
//! when the thread root is deleted. Mirrors the thread_followers suite.

use lets_chat::db;

mod common;

#[tokio::test]
async fn mute_unmute_roundtrip() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();

    assert!(!db::thread_muters::is_muted(&chat, "u2", root)
        .await
        .unwrap());
    db::thread_muters::mute(&chat, "u2", root, 1).await.unwrap();
    assert!(db::thread_muters::is_muted(&chat, "u2", root)
        .await
        .unwrap());

    db::thread_muters::unmute(&chat, "u2", root).await.unwrap();
    assert!(!db::thread_muters::is_muted(&chat, "u2", root)
        .await
        .unwrap());
}

#[tokio::test]
async fn mute_is_idempotent_and_muters_lists_all() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();
    db::thread_muters::mute(&chat, "a", root, 1).await.unwrap();
    db::thread_muters::mute(&chat, "a", root, 1).await.unwrap(); // no-op
    db::thread_muters::mute(&chat, "b", root, 1).await.unwrap();

    let mut m = db::thread_muters::muters(&chat, root).await.unwrap();
    m.sort();
    assert_eq!(m, vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn deleting_root_cascades_muters() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();
    db::thread_muters::mute(&chat, "a", root, 1).await.unwrap();

    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(root)
        .execute(&chat)
        .await
        .unwrap();

    assert!(db::thread_muters::muters(&chat, root)
        .await
        .unwrap()
        .is_empty());
}
