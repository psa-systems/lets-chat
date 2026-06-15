//! LC-304: notification_keywords db layer - add/list/remove/dedup/count.

use lets_chat::db;

mod common;

#[tokio::test]
async fn add_list_remove_roundtrip() {
    let auth = common::pool("auth").await;
    let uid = db::auth::create_user(&auth, "alice", "h").await.unwrap();

    db::notification_keywords::add(&auth, &uid, "deploy")
        .await
        .unwrap();
    db::notification_keywords::add(&auth, &uid, "on-call")
        .await
        .unwrap();
    let words = db::notification_keywords::list(&auth, &uid).await.unwrap();
    assert_eq!(words, vec!["deploy".to_string(), "on-call".to_string()]);

    db::notification_keywords::remove(&auth, &uid, "deploy")
        .await
        .unwrap();
    let words = db::notification_keywords::list(&auth, &uid).await.unwrap();
    assert_eq!(words, vec!["on-call".to_string()]);
}

#[tokio::test]
async fn add_is_idempotent() {
    let auth = common::pool("auth").await;
    let uid = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    db::notification_keywords::add(&auth, &uid, "urgent")
        .await
        .unwrap();
    db::notification_keywords::add(&auth, &uid, "urgent")
        .await
        .unwrap();
    assert_eq!(
        db::notification_keywords::count(&auth, &uid).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn all_returns_pairs_across_users() {
    let auth = common::pool("auth").await;
    let a = db::auth::create_user(&auth, "ann", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "ben", "h").await.unwrap();
    db::notification_keywords::add(&auth, &a, "alpha")
        .await
        .unwrap();
    db::notification_keywords::add(&auth, &b, "beta")
        .await
        .unwrap();

    let all = db::notification_keywords::all(&auth).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.contains(&(a, "alpha".to_string())));
    assert!(all.contains(&(b, "beta".to_string())));
}
