use lets_chat::db::bookmarks::{
    bookmark_message, bookmarked_message_ids_in_room, bookmarks_for_user, is_bookmarked,
    room_for_message, set_label, unbookmark_message,
};
use sqlx::SqlitePool;
mod common;

async fn setup_chat_pool() -> sqlx::SqlitePool {
    common::chat_pool().await
}

async fn seed_room(pool: &SqlitePool, name: &str, kind: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, ?)")
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn seed_message(pool: &SqlitePool, room_id: i64, user_id: &str, body: &str) -> i64 {
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

#[tokio::test]
async fn bookmark_then_unbookmark_round_trips() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    assert!(is_bookmarked(&pool, "viewer", msg).await.unwrap());

    unbookmark_message(&pool, "viewer", msg).await.unwrap();
    assert!(!is_bookmarked(&pool, "viewer", msg).await.unwrap());
}

#[tokio::test]
async fn set_label_roundtrips_and_clears() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;
    bookmark_message(&pool, "viewer", msg).await.unwrap();

    // Default: unlabeled.
    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, None);

    // Set a label.
    set_label(&pool, "viewer", msg, Some("follow-up"))
        .await
        .unwrap();
    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows[0].label.as_deref(), Some("follow-up"));

    // Clear it back to NULL.
    set_label(&pool, "viewer", msg, None).await.unwrap();
    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows[0].label, None);
}

#[tokio::test]
async fn set_label_is_scoped_to_owner() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;
    bookmark_message(&pool, "alice", msg).await.unwrap();
    bookmark_message(&pool, "bob", msg).await.unwrap();

    set_label(&pool, "alice", msg, Some("read-later"))
        .await
        .unwrap();

    let alice = bookmarks_for_user(&pool, "alice").await.unwrap();
    assert_eq!(alice[0].label.as_deref(), Some("read-later"));
    let bob = bookmarks_for_user(&pool, "bob").await.unwrap();
    assert_eq!(bob[0].label, None, "bob's bookmark is untouched");
}

#[tokio::test]
async fn bookmark_is_idempotent() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    bookmark_message(&pool, "viewer", msg).await.unwrap();

    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn unbookmark_nonexistent_is_a_noop() {
    let pool = setup_chat_pool().await;
    unbookmark_message(&pool, "viewer", 99999).await.unwrap();
}

#[tokio::test]
async fn bookmarks_are_private_per_user() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "alice", msg).await.unwrap();
    assert!(is_bookmarked(&pool, "alice", msg).await.unwrap());
    assert!(!is_bookmarked(&pool, "bob", msg).await.unwrap());

    let alice_rows = bookmarks_for_user(&pool, "alice").await.unwrap();
    assert_eq!(alice_rows.len(), 1);
    let bob_rows = bookmarks_for_user(&pool, "bob").await.unwrap();
    assert!(bob_rows.is_empty());
}

#[tokio::test]
async fn bookmarks_for_user_excludes_soft_deleted_messages() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let kept = seed_message(&pool, room, "author", "kept").await;
    let gone = seed_message(&pool, room, "author", "gone").await;

    bookmark_message(&pool, "viewer", kept).await.unwrap();
    bookmark_message(&pool, "viewer", gone).await.unwrap();

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    let rows = bookmarks_for_user(&pool, "viewer").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, kept);
}

#[tokio::test]
async fn bookmarked_message_ids_in_room_is_room_scoped() {
    let pool = setup_chat_pool().await;
    let room_a = seed_room(&pool, "alpha", "public").await;
    let room_b = seed_room(&pool, "beta", "public").await;
    let in_a = seed_message(&pool, room_a, "author", "a").await;
    let in_b = seed_message(&pool, room_b, "author", "b").await;

    bookmark_message(&pool, "viewer", in_a).await.unwrap();
    bookmark_message(&pool, "viewer", in_b).await.unwrap();

    let a_ids = bookmarked_message_ids_in_room(&pool, "viewer", room_a)
        .await
        .unwrap();
    assert!(a_ids.contains(&in_a));
    assert!(!a_ids.contains(&in_b));
}

#[tokio::test]
async fn cascading_delete_of_message_removes_bookmark_row() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    bookmark_message(&pool, "viewer", msg).await.unwrap();
    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(msg)
        .execute(&pool)
        .await
        .unwrap();

    assert!(!is_bookmarked(&pool, "viewer", msg).await.unwrap());
}

#[tokio::test]
async fn room_for_message_returns_none_for_soft_deleted() {
    let pool = setup_chat_pool().await;
    let room = seed_room(&pool, "general", "public").await;
    let msg = seed_message(&pool, room, "author", "hi").await;

    assert_eq!(room_for_message(&pool, msg).await.unwrap(), Some(room));

    sqlx::query("UPDATE messages SET deleted_at = datetime('now') WHERE id = ?")
        .bind(msg)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(room_for_message(&pool, msg).await.unwrap(), None);
}
