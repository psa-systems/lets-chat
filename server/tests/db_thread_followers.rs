//! LC-310: thread_followers db layer - follow/unfollow/is_following/followers
//! and FK cascade when the thread root is deleted.

use lets_chat::db;

mod common;

#[tokio::test]
async fn follow_unfollow_roundtrip() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();

    assert!(!db::thread_followers::is_following(&chat, "u2", root)
        .await
        .unwrap());
    db::thread_followers::follow(&chat, "u2", root, 1)
        .await
        .unwrap();
    assert!(db::thread_followers::is_following(&chat, "u2", root)
        .await
        .unwrap());

    db::thread_followers::unfollow(&chat, "u2", root)
        .await
        .unwrap();
    assert!(!db::thread_followers::is_following(&chat, "u2", root)
        .await
        .unwrap());
}

#[tokio::test]
async fn follow_is_idempotent_and_followers_lists_all() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();
    db::thread_followers::follow(&chat, "a", root, 1)
        .await
        .unwrap();
    db::thread_followers::follow(&chat, "a", root, 1)
        .await
        .unwrap(); // no-op
    db::thread_followers::follow(&chat, "b", root, 1)
        .await
        .unwrap();

    let mut f = db::thread_followers::followers(&chat, root).await.unwrap();
    f.sort();
    assert_eq!(f, vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn deleting_root_cascades_followers() {
    let chat = common::pool("chat").await;
    let root = db::chat::insert_message(&chat, 1, "u1", "root")
        .await
        .unwrap();
    db::thread_followers::follow(&chat, "a", root, 1)
        .await
        .unwrap();

    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(root)
        .execute(&chat)
        .await
        .unwrap();

    assert!(db::thread_followers::followers(&chat, root)
        .await
        .unwrap()
        .is_empty());
}
