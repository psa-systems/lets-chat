//! LC-526: kudos leaderboard (db layer).

use lets_chat::db::kudos::{record, top_givers, top_receivers};

mod common;

#[tokio::test]
async fn leaderboard_ranks_and_scopes_to_enclave() {
    let pool = common::chat_pool().await;
    // Enclave 1: alice receives 2 (from bob, carol), bob receives 1 (from alice).
    record(&pool, "bob", "alice", 10, Some(1), Some("x"), None)
        .await
        .unwrap();
    record(&pool, "carol", "alice", 10, Some(1), None, None)
        .await
        .unwrap();
    record(&pool, "alice", "bob", 10, Some(1), None, None)
        .await
        .unwrap();
    // Enclave 2: dave receives 5 - must not leak into an enclave-1 query.
    for _ in 0..5 {
        record(&pool, "x", "dave", 20, Some(2), None, None)
            .await
            .unwrap();
    }

    let recv = top_receivers(&pool, &[1], "-30 days", 10, &[]).await.unwrap();
    assert_eq!(recv.len(), 2);
    assert_eq!(recv[0].user_id, "alice");
    assert_eq!(recv[0].count, 2);
    assert_eq!(recv[1].user_id, "bob");
    assert_eq!(recv[1].count, 1);
    assert!(recv.iter().all(|l| l.user_id != "dave"));

    let give = top_givers(&pool, &[1], "-30 days", 10, &[]).await.unwrap();
    assert_eq!(give.len(), 3); // bob, carol, alice each gave once
    assert_eq!(give.iter().map(|l| l.count).sum::<i64>(), 3);

    // Scoping to enclave 2 yields only dave.
    let recv2 = top_receivers(&pool, &[2], "-30 days", 10, &[]).await.unwrap();
    assert_eq!(recv2.len(), 1);
    assert_eq!(recv2[0].user_id, "dave");
    assert_eq!(recv2[0].count, 5);

    // No enclaves -> nothing.
    assert!(top_receivers(&pool, &[], "-30 days", 10, &[])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn window_excludes_old_kudos() {
    let pool = common::chat_pool().await;
    record(&pool, "bob", "alice", 10, Some(1), None, None)
        .await
        .unwrap();
    // An old kudos (well outside 30 days) via raw insert must not count.
    sqlx::query(
        "INSERT INTO kudos (giver_id, receiver_id, room_id, enclave_id, created_at) \
         VALUES ('bob', 'carol', 10, 1, '2000-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let recv = top_receivers(&pool, &[1], "-30 days", 10, &[]).await.unwrap();
    assert_eq!(recv.len(), 1);
    assert_eq!(recv[0].user_id, "alice");
}

#[tokio::test]
async fn respects_limit() {
    let pool = common::chat_pool().await;
    for i in 0..5 {
        record(&pool, "g", &format!("r{i}"), 10, Some(1), None, None)
            .await
            .unwrap();
    }
    let recv = top_receivers(&pool, &[1], "-30 days", 3, &[]).await.unwrap();
    assert_eq!(recv.len(), 3);
}
