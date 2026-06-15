//! LC-312: saved_searches db layer - add/list/remove/dedup, recent-first order.

use lets_chat::db;

mod common;

#[tokio::test]
async fn add_list_remove_roundtrip() {
    let chat = common::pool("chat").await;
    db::saved_searches::add(&chat, "u1", "from:alice deploy")
        .await
        .unwrap();
    let list = db::saved_searches::list(&chat, "u1").await.unwrap();
    assert_eq!(list, vec!["from:alice deploy".to_string()]);

    db::saved_searches::remove(&chat, "u1", "from:alice deploy")
        .await
        .unwrap();
    assert!(db::saved_searches::list(&chat, "u1")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn add_is_idempotent() {
    let chat = common::pool("chat").await;
    db::saved_searches::add(&chat, "u1", "cats").await.unwrap();
    db::saved_searches::add(&chat, "u1", "cats").await.unwrap();
    assert_eq!(db::saved_searches::count(&chat, "u1").await.unwrap(), 1);
}

#[tokio::test]
async fn list_is_per_user() {
    let chat = common::pool("chat").await;
    db::saved_searches::add(&chat, "a", "alpha").await.unwrap();
    db::saved_searches::add(&chat, "b", "beta").await.unwrap();
    assert_eq!(
        db::saved_searches::list(&chat, "a").await.unwrap(),
        vec!["alpha".to_string()]
    );
    assert_eq!(
        db::saved_searches::list(&chat, "b").await.unwrap(),
        vec!["beta".to_string()]
    );
}
