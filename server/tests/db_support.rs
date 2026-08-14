//! LC-714: support-ticket db layer (create, open queue, single-handle resolve).
//! Mirrors db_reports.rs.

use lets_chat::db;

mod common;

#[tokio::test]
async fn create_returns_id_and_lists_open_newest_first() {
    let chat = common::chat_pool().await;
    let a = db::support_tickets::create(&chat, "user1", Some(1), "general", "help me")
        .await
        .expect("create a");
    let b = db::support_tickets::create(&chat, "user2", None, "", "and me")
        .await
        .expect("create b");
    assert!(b > a, "ids increase");

    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 2);
    let open = db::support_tickets::list_open(&chat).await.unwrap();
    assert_eq!(open.len(), 2);
    // Newest first (b before a).
    assert_eq!(open[0].id, b);
    assert_eq!(open[0].requester_id, "user2");
    assert_eq!(open[0].room_id, None);
    assert_eq!(open[1].id, a);
    assert_eq!(open[1].room_id, Some(1));
    assert_eq!(open[1].body, "help me");
}

#[tokio::test]
async fn set_status_resolves_once() {
    let chat = common::chat_pool().await;
    let id = db::support_tickets::create(&chat, "user1", Some(1), "general", "help")
        .await
        .unwrap();
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 1);

    let first = db::support_tickets::set_status(&chat, id, "resolved", "admin1")
        .await
        .unwrap();
    assert!(first, "first resolve transitions the open ticket");
    assert_eq!(db::support_tickets::count_open(&chat).await.unwrap(), 0);
    assert!(db::support_tickets::list_open(&chat)
        .await
        .unwrap()
        .is_empty());

    // A second resolve on the already-resolved ticket is a no-op (no double-handle).
    let second = db::support_tickets::set_status(&chat, id, "resolved", "admin2")
        .await
        .unwrap();
    assert!(!second, "already-resolved ticket does not transition again");

    // The resolved ticket is still fetchable, with its handler recorded.
    let t = db::support_tickets::get(&chat, id).await.unwrap().unwrap();
    assert_eq!(t.status, "resolved");
    assert_eq!(t.handled_by.as_deref(), Some("admin1"));
}

#[tokio::test]
async fn body_is_length_bounded() {
    let chat = common::chat_pool().await;
    let long = "x".repeat(db::support_tickets::MAX_TICKET_BODY_CHARS + 500);
    let id = db::support_tickets::create(&chat, "user1", None, "", &long)
        .await
        .unwrap();
    let t = db::support_tickets::get(&chat, id).await.unwrap().unwrap();
    assert_eq!(
        t.body.chars().count(),
        db::support_tickets::MAX_TICKET_BODY_CHARS
    );
}
