//! LC-342: shame-tag DB invariants - vote toggle, per-tag counts with aging,
//! the hide threshold, and moderator-override precedence. Uses the migrate!-
//! backed common chat pool so migration 0065 applies automatically.

mod common;

use lets_chat::db;
use lets_chat::db::shame_tags::HiddenState;
use sqlx::SqlitePool;

async fn message(pool: &SqlitePool) -> i64 {
    let room = db::chat::create_room(pool, "r", None, "public", None, None)
        .await
        .unwrap();
    db::chat::insert_message(pool, room, "author", "hi")
        .await
        .unwrap()
}

async fn vote(pool: &SqlitePool, mid: i64, tag: &str, voter: &str) {
    db::shame_tags::toggle_vote(pool, mid, tag, voter)
        .await
        .unwrap();
}

#[tokio::test]
async fn vote_toggles_and_counts() {
    let pool = common::chat_pool().await;
    let mid = message(&pool).await;

    assert!(db::shame_tags::toggle_vote(&pool, mid, "spam", "v1")
        .await
        .unwrap());
    assert_eq!(
        db::shame_tags::tag_counts(&pool, mid)
            .await
            .unwrap()
            .get("spam"),
        Some(&1)
    );
    assert_eq!(
        db::shame_tags::voter_tags(&pool, mid, "v1").await.unwrap(),
        vec!["spam"]
    );
    // Toggling again removes it.
    assert!(!db::shame_tags::toggle_vote(&pool, mid, "spam", "v1")
        .await
        .unwrap());
    assert!(db::shame_tags::tag_counts(&pool, mid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn threshold_hides_only_hide_worthy_tags() {
    let pool = common::chat_pool().await;
    let mid = message(&pool).await;

    // 2 spam votes: below threshold (3) -> visible.
    vote(&pool, mid, "spam", "v1").await;
    vote(&pool, mid, "spam", "v2").await;
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid).await.unwrap(),
        None
    );

    // 3rd distinct voter -> hidden by the "spam" tag, not a moderator.
    vote(&pool, mid, "spam", "v3").await;
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid).await.unwrap(),
        Some(HiddenState {
            reason: "spam".to_string(),
            by_moderator: false
        })
    );

    // A non-hide-worthy tag never hides, even past threshold.
    let mid2 = message(&pool).await;
    for v in ["v1", "v2", "v3", "v4"] {
        vote(&pool, mid2, "off-topic", v).await;
    }
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid2).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn aged_out_votes_do_not_count() {
    let pool = common::chat_pool().await;
    let mid = message(&pool).await;
    // Two fresh, one 40-days-old: only 2 in-window -> not hidden.
    vote(&pool, mid, "spam", "v1").await;
    vote(&pool, mid, "spam", "v2").await;
    sqlx::query(
        "INSERT INTO message_tags (message_id, tag, voter_user_id, created_at) \
         VALUES (?, 'spam', 'old', datetime('now','-40 days'))",
    )
    .bind(mid)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn moderator_override_wins() {
    let pool = common::chat_pool().await;
    let mid = message(&pool).await;
    for v in ["v1", "v2", "v3"] {
        vote(&pool, mid, "spam", v).await; // would hide by vote
    }

    // Force-show suppresses the community hide.
    db::shame_tags::set_override(&pool, mid, false, "mod")
        .await
        .unwrap();
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid).await.unwrap(),
        None
    );

    // Force-hide hides (reason = moderator) even with no votes.
    let mid2 = message(&pool).await;
    db::shame_tags::set_override(&pool, mid2, true, "mod")
        .await
        .unwrap();
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid2).await.unwrap(),
        Some(HiddenState {
            reason: "moderator".to_string(),
            by_moderator: true
        })
    );

    // Clearing reverts mid to the community decision (hidden by votes again).
    db::shame_tags::clear_override(&pool, mid).await.unwrap();
    assert_eq!(
        db::shame_tags::hidden_state(&pool, mid).await.unwrap(),
        Some(HiddenState {
            reason: "spam".to_string(),
            by_moderator: false
        })
    );
}

#[tokio::test]
async fn batch_returns_only_hidden() {
    let pool = common::chat_pool().await;
    let hidden = message(&pool).await;
    let visible = message(&pool).await;
    for v in ["v1", "v2", "v3"] {
        vote(&pool, hidden, "abusive", v).await;
    }
    vote(&pool, visible, "abusive", "v1").await; // below threshold

    let map = db::shame_tags::hidden_states_for_messages(&pool, &[hidden, visible])
        .await
        .unwrap();
    assert!(map.contains_key(&hidden));
    assert!(!map.contains_key(&visible));
    assert_eq!(map.get(&hidden).unwrap().reason, "abusive");
}
