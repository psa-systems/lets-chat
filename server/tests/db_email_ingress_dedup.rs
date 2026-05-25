//! LC-77-MID-DEDUP commit 1: schema round-trip + sweep behavior tests
//! for the `processed_message_ids` dedup table.

use lets_chat::db::email_ingress_dedup as dedup;

mod common;

const SECRET: [u8; 32] = [7u8; 32];

#[tokio::test]
async fn hash_message_id_is_stable_and_keyed() {
    // Same input + same key -> same hash.
    let a = dedup::hash_message_id(&SECRET, "<abc@example.com>");
    let b = dedup::hash_message_id(&SECRET, "<abc@example.com>");
    assert_eq!(a, b);
    // Different input -> different hash.
    let c = dedup::hash_message_id(&SECRET, "<xyz@example.com>");
    assert_ne!(a, c);
    // Different key -> different hash for the same input.
    let d = dedup::hash_message_id(&[8u8; 32], "<abc@example.com>");
    assert_ne!(a, d);
}

#[tokio::test]
async fn is_processed_returns_false_for_unknown_hash() {
    let chat = common::chat_pool().await;
    let hash = dedup::hash_message_id(&SECRET, "<never-seen@example.com>");
    assert!(!dedup::is_processed(&chat, &hash).await.unwrap());
}

#[tokio::test]
async fn mark_processed_then_is_processed_round_trips() {
    let chat = common::chat_pool().await;
    let hash = dedup::hash_message_id(&SECRET, "<fresh@example.com>");

    let inserted = dedup::mark_processed(&chat, &hash).await.unwrap();
    assert!(inserted, "first mark must report a new row was inserted");
    assert!(dedup::is_processed(&chat, &hash).await.unwrap());
}

#[tokio::test]
async fn mark_processed_is_idempotent() {
    // Two concurrent polls against the same mailbox could both reach
    // `mark_processed` for the same Message-ID. `INSERT OR IGNORE`
    // makes the second call a no-op; the second caller learns
    // `inserted = false` and can log a duplicate-detected warning
    // without aborting.
    let chat = common::chat_pool().await;
    let hash = dedup::hash_message_id(&SECRET, "<idempotent@example.com>");
    assert!(dedup::mark_processed(&chat, &hash).await.unwrap());
    assert!(
        !dedup::mark_processed(&chat, &hash).await.unwrap(),
        "second mark must report no row was inserted",
    );
    // The row is still queryable.
    assert!(dedup::is_processed(&chat, &hash).await.unwrap());
}

#[tokio::test]
async fn sweep_old_drops_only_past_cutoff_rows() {
    let chat = common::chat_pool().await;
    let fresh = dedup::hash_message_id(&SECRET, "<fresh-row@example.com>");
    let stale = dedup::hash_message_id(&SECRET, "<stale-row@example.com>");

    // Insert one row with the default `processed_at = datetime('now')`
    // and a second row with a hand-set timestamp older than the
    // 30-day cutoff. `sweep_old(30)` must drop only the stale one.
    dedup::mark_processed(&chat, &fresh).await.unwrap();
    sqlx::query("INSERT INTO processed_message_ids (message_id_hash, processed_at) VALUES (?, ?)")
        .bind(&stale)
        .bind("2000-01-01 00:00:00")
        .execute(&chat)
        .await
        .unwrap();

    let dropped = dedup::sweep_old(&chat, 30).await.unwrap();
    assert_eq!(dropped, 1, "sweep_old(30) must drop exactly the stale row");
    assert!(dedup::is_processed(&chat, &fresh).await.unwrap());
    assert!(!dedup::is_processed(&chat, &stale).await.unwrap());
}
