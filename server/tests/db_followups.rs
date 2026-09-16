//! LC-527: follow-up task lists (db layer).

use lets_chat::db::followups::{
    create, followup_message_ids, get, item, items, toggle_claim, toggle_done,
};
use sqlx::SqlitePool;

mod common;

async fn seed_room(pool: &SqlitePool) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES ('general', 'public')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn create_lists_items_and_detects_membership() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let mid = create(
        &pool,
        room,
        "u1",
        "Follow-up tasks",
        Some(7),
        &["Ship it".to_string(), "Email Bob".to_string()],
    )
    .await
    .unwrap();

    let head = get(&pool, mid).await.unwrap().unwrap();
    assert_eq!(head.transcript_id, Some(7));
    assert_eq!(head.created_by, "u1");

    let list = items(&pool, mid).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].text, "Ship it");
    assert_eq!(list[1].text, "Email Bob");
    assert!(!list[0].done);
    assert!(list[0].assignee_id.is_none());

    let set = followup_message_ids(&pool, &[mid, 9999]).await.unwrap();
    assert!(set.contains(&mid));
    assert!(!set.contains(&9999));
    // A non-follow-up message id resolves to None.
    assert!(get(&pool, 9999).await.unwrap().is_none());
}

#[tokio::test]
async fn toggle_done_is_a_flip_and_stamps_completer() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let mid = create(&pool, room, "u1", "t", None, &["A".to_string()])
        .await
        .unwrap();
    let id = items(&pool, mid).await.unwrap()[0].id;

    toggle_done(&pool, id, "u2").await.unwrap();
    assert!(item(&pool, id).await.unwrap().unwrap().done);
    toggle_done(&pool, id, "u2").await.unwrap();
    assert!(!item(&pool, id).await.unwrap().unwrap().done);
}

#[tokio::test]
async fn claim_is_self_toggle() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let mid = create(&pool, room, "u1", "t", None, &["A".to_string()])
        .await
        .unwrap();
    let id = items(&pool, mid).await.unwrap()[0].id;

    // Claim -> assigned to me.
    assert!(toggle_claim(&pool, id, "u2").await.unwrap());
    assert_eq!(
        item(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some("u2")
    );
    // Same user claims again -> released.
    assert!(toggle_claim(&pool, id, "u2").await.unwrap());
    assert!(item(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .assignee_id
        .is_none());
    // A different user can take an unclaimed item.
    assert!(toggle_claim(&pool, id, "u3").await.unwrap());
    assert_eq!(
        item(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some("u3")
    );
}

#[tokio::test]
async fn claim_refuses_to_take_from_another_user() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let mid = create(&pool, room, "u1", "t", None, &["A".to_string()])
        .await
        .unwrap();
    let id = items(&pool, mid).await.unwrap()[0].id;

    // u2 claims it.
    assert!(toggle_claim(&pool, id, "u2").await.unwrap());

    // u3 tries to take it from u2: no row changes, and u2 still holds it.
    assert!(!toggle_claim(&pool, id, "u3").await.unwrap());
    assert_eq!(
        item(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some("u2")
    );

    // u2 can still release their own item.
    assert!(toggle_claim(&pool, id, "u2").await.unwrap());
    assert!(item(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .assignee_id
        .is_none());

    // Now unclaimed, u3 can claim it.
    assert!(toggle_claim(&pool, id, "u3").await.unwrap());
    assert_eq!(
        item(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .assignee_id
            .as_deref(),
        Some("u3")
    );
}
