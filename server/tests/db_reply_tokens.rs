//! LC-77-REPLY commit 1a: schema round-trip + behavior tests for the
//! `reply_tokens` side table that backs reply-by-email.

use lets_chat::db;

mod common;

async fn fresh_room(chat: &sqlx::SqlitePool) -> i64 {
    db::chat::create_room(chat, "ops", None, "public", None, None)
        .await
        .unwrap()
}

async fn insert_message(chat: &sqlx::SqlitePool, room_id: i64, user_id: &str) -> i64 {
    db::chat::insert_message(chat, room_id, user_id, "hi")
        .await
        .unwrap()
}

#[tokio::test]
async fn mint_token_returns_distinct_base32_strings() {
    // Sanity: 32 random bytes base32-encoded into ~52 chars; two calls
    // must produce different tokens with overwhelming probability.
    let a = db::reply_tokens::mint_token();
    let b = db::reply_tokens::mint_token();
    assert_ne!(a, b);
    assert!(a.len() >= 50);
    assert!(a
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    assert!(b
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
}

#[tokio::test]
async fn insert_then_resolve_round_trips_user_and_message_id() {
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;

    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(&chat, &token, "alice", message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();

    let row = db::reply_tokens::resolve(&chat, &token)
        .await
        .unwrap()
        .expect("resolve must return Some for the just-inserted token");
    assert_eq!(row.user_id, "alice");
    assert_eq!(row.message_id, message_id);
    assert_eq!(row.expires_at, "2099-01-01 00:00:00");
}

#[tokio::test]
async fn resolve_unknown_token_returns_none() {
    let chat = common::chat_pool().await;
    let out = db::reply_tokens::resolve(&chat, "DEFINITELY-NOT-A-REAL-TOKEN")
        .await
        .unwrap();
    assert!(out.is_none());
}

#[tokio::test]
async fn sweep_expired_drops_past_dated_rows_only() {
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;

    let stale = db::reply_tokens::mint_token();
    let fresh = db::reply_tokens::mint_token();

    db::reply_tokens::insert(&chat, &stale, "alice", message_id, "2000-01-01 00:00:00")
        .await
        .unwrap();
    db::reply_tokens::insert(&chat, &fresh, "alice", message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();

    let dropped = db::reply_tokens::sweep_expired(&chat).await.unwrap();
    assert_eq!(dropped, 1);

    assert!(db::reply_tokens::resolve(&chat, &stale)
        .await
        .unwrap()
        .is_none());
    assert!(db::reply_tokens::resolve(&chat, &fresh)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn message_delete_cascades_reply_tokens() {
    // ON DELETE CASCADE on message_id: deleting the original message
    // reaps any outstanding reply tokens. A reply to a deleted message
    // resolves to None (same outcome as an unknown token).
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;

    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(&chat, &token, "alice", message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();

    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(message_id)
        .execute(&chat)
        .await
        .unwrap();

    let resolved = db::reply_tokens::resolve(&chat, &token).await.unwrap();
    assert!(
        resolved.is_none(),
        "deleting the original message must cascade and drop the reply token",
    );
}

#[tokio::test]
async fn notify_email_activity_enabled_round_trips_via_setter() {
    let auth = common::auth_pool().await;
    let user_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();

    // Default is 0 (per migration).
    let user = db::auth::find_user_by_id(&auth, &user_id)
        .await
        .unwrap()
        .expect("user row");
    assert!(!user.notify_email_activity_enabled);

    db::auth::set_notify_email_activity_enabled(&auth, &user_id, true)
        .await
        .unwrap();
    let user = db::auth::find_user_by_id(&auth, &user_id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.notify_email_activity_enabled);

    db::auth::set_notify_email_activity_enabled(&auth, &user_id, false)
        .await
        .unwrap();
    let user = db::auth::find_user_by_id(&auth, &user_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!user.notify_email_activity_enabled);
}
