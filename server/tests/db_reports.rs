//! LC-334: message-report db layer (create idempotency, open queue, handling).

use lets_chat::db;

mod common;

async fn a_message(chat: &sqlx::SqlitePool) -> (i64, i64) {
    // Seeded rooms exist from chat migration 0001; post into room 1.
    let room_id = 1;
    let id = db::chat::insert_message(chat, room_id, "author", "a bad message")
        .await
        .expect("insert message");
    (id, room_id)
}

#[tokio::test]
async fn create_is_idempotent_per_reporter() {
    let chat = common::chat_pool().await;
    let (mid, rid) = a_message(&chat).await;

    let first = db::reports::create(&chat, mid, rid, "alice", "spam", Some("buy stuff"))
        .await
        .expect("create");
    assert!(first, "first report inserts");

    // Same reporter, same message: suppressed by UNIQUE, no second row.
    let second = db::reports::create(&chat, mid, rid, "alice", "harassment", None)
        .await
        .expect("create dup");
    assert!(!second, "duplicate report is a no-op");

    // A different reporter on the same message is a distinct report.
    let other = db::reports::create(&chat, mid, rid, "bob", "other", None)
        .await
        .expect("create other");
    assert!(other);

    assert_eq!(db::reports::count_open(&chat).await.unwrap(), 2);
}

#[tokio::test]
async fn list_open_returns_open_only_and_set_status_handles_once() {
    let chat = common::chat_pool().await;
    let (mid, rid) = a_message(&chat).await;
    db::reports::create(&chat, mid, rid, "alice", "spam", None)
        .await
        .unwrap();

    let open = db::reports::list_open(&chat).await.unwrap();
    assert_eq!(open.len(), 1);
    let report_id = open[0].id;
    assert_eq!(open[0].status, "open");

    // Resolve transitions it out of the open set.
    let handled = db::reports::set_status(&chat, report_id, "resolved", "mod1")
        .await
        .unwrap();
    assert!(handled);
    assert_eq!(db::reports::count_open(&chat).await.unwrap(), 0);
    assert!(db::reports::list_open(&chat).await.unwrap().is_empty());

    // A second handler on the now-resolved report is a no-op (no double-handle).
    let again = db::reports::set_status(&chat, report_id, "dismissed", "mod2")
        .await
        .unwrap();
    assert!(!again);
}

#[test]
fn category_allowlist() {
    assert!(db::reports::is_valid_category("spam"));
    assert!(db::reports::is_valid_category("harassment"));
    assert!(db::reports::is_valid_category("inappropriate"));
    assert!(db::reports::is_valid_category("other"));
    assert!(!db::reports::is_valid_category("nonsense"));
    assert!(!db::reports::is_valid_category(""));
}
