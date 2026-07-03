//! LC-529: reaction highlights recap (db layer).

use lets_chat::db::highlights::top_reacted;
use sqlx::SqlitePool;

mod common;

async fn seed_room(pool: &SqlitePool) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES ('general', 'public')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn seed_message(pool: &SqlitePool, room_id: i64, body: &str) -> i64 {
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, 'author', ?)")
        .bind(room_id)
        .bind(body)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn react(pool: &SqlitePool, message_id: i64, user: &str, emoji: &str, created_at: &str) {
    sqlx::query(
        "INSERT INTO message_reactions (message_id, user_id, emoji, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(user)
    .bind(emoji)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn ranks_by_recent_reaction_count_with_emoji_breakdown() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let m1 = seed_message(&pool, room, "popular").await;
    let m2 = seed_message(&pool, room, "some").await;
    let _m3 = seed_message(&pool, room, "none").await;

    // m1: 2x thumbs + 1 heart = 3; m2: 1 thumbs. A future-dated timestamp is
    // trivially inside the "-7 days" window (lexicographic == chronological for
    // SQLite's `YYYY-MM-DD HH:MM:SS` format).
    for (mid, user, emoji) in [
        (m1, "a", "\u{1f44d}"),
        (m1, "b", "\u{1f44d}"),
        (m1, "c", "\u{2764}\u{fe0f}"),
        (m2, "a", "\u{1f44d}"),
    ] {
        react(&pool, mid, user, emoji, "2999-01-01 00:00:00").await;
    }

    let rows = top_reacted(&pool, room, "-7 days", 10).await.unwrap();
    // Future-dated reactions are still >= now-7days, so both messages appear,
    // m1 first (3 > 1); the reaction-less message is excluded.
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].message_id, m1);
    assert_eq!(rows[0].total, 3);
    assert_eq!(rows[1].message_id, m2);
    assert_eq!(rows[1].total, 1);

    // Emoji breakdown for m1: thumbs (2) ranks before heart (1).
    assert_eq!(rows[0].emojis.len(), 2);
    assert_eq!(rows[0].emojis[0], ("\u{1f44d}".to_string(), 2));
    assert_eq!(rows[0].emojis[1], ("\u{2764}\u{fe0f}".to_string(), 1));
}

#[tokio::test]
async fn excludes_reactions_outside_the_window() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    let old = seed_message(&pool, room, "old news").await;
    let fresh = seed_message(&pool, room, "fresh").await;

    // Old reaction (well outside 7 days) must not surface its message.
    react(&pool, old, "a", "\u{1f44d}", "2000-01-01 00:00:00").await;
    // Recent reaction.
    react(&pool, fresh, "a", "\u{1f44d}", "2999-01-01 00:00:00").await;

    let rows = top_reacted(&pool, room, "-7 days", 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, fresh);
}

#[tokio::test]
async fn respects_the_limit() {
    let pool = common::chat_pool().await;
    let room = seed_room(&pool).await;
    for i in 0..5 {
        let m = seed_message(&pool, room, &format!("msg {i}")).await;
        react(&pool, m, "a", "\u{1f44d}", "2999-01-01 00:00:00").await;
    }
    let rows = top_reacted(&pool, room, "-7 days", 3).await.unwrap();
    assert_eq!(rows.len(), 3);
}
