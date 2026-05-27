//! LC-186: remote-control session audit storage.

mod common;

use lets_chat::db::remote_control_audit as audit;
use sqlx::Row;

async fn open_rows(pool: &sqlx::SqlitePool, room_id: i64) -> i64 {
    sqlx::query("SELECT COUNT(*) AS n FROM remote_control_sessions WHERE room_id = ? AND ended_at IS NULL")
        .bind(room_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<i64, _>("n")
}

#[tokio::test]
async fn start_then_end_by_room() {
    let pool = common::chat_pool().await;
    audit::start_session(&pool, 7, "controller", "sharer").await.unwrap();
    assert_eq!(open_rows(&pool, 7).await, 1);

    audit::end_session_by_room(&pool, 7, "revoked").await.unwrap();
    assert_eq!(open_rows(&pool, 7).await, 0);

    // The closed row carries the participants + reason.
    let row = sqlx::query(
        "SELECT controller_id, sharer_id, end_reason, ended_at FROM remote_control_sessions WHERE room_id = ?",
    )
    .bind(7)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("controller_id"), "controller");
    assert_eq!(row.get::<String, _>("sharer_id"), "sharer");
    assert_eq!(row.get::<String, _>("end_reason"), "revoked");
    assert!(row.get::<Option<String>, _>("ended_at").is_some());
}

#[tokio::test]
async fn start_is_idempotent_while_open() {
    let pool = common::chat_pool().await;
    audit::start_session(&pool, 1, "c", "s").await.unwrap();
    // A repeated grant must not stack a second open row.
    audit::start_session(&pool, 1, "c", "s").await.unwrap();
    assert_eq!(open_rows(&pool, 1).await, 1);

    // After a close, a fresh grant opens a new row.
    audit::end_session_by_room(&pool, 1, "revoked").await.unwrap();
    audit::start_session(&pool, 1, "c", "s").await.unwrap();
    assert_eq!(open_rows(&pool, 1).await, 1);
    let total: i64 = sqlx::query("SELECT COUNT(*) AS n FROM remote_control_sessions WHERE room_id = ?")
        .bind(1)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(total, 2);
}

#[tokio::test]
async fn end_for_user_closes_both_roles() {
    let pool = common::chat_pool().await;
    // alice controls in room 10; alice is shared-to in room 11.
    audit::start_session(&pool, 10, "alice", "bob").await.unwrap();
    audit::start_session(&pool, 11, "carol", "alice").await.unwrap();
    // An unrelated session must survive.
    audit::start_session(&pool, 12, "bob", "carol").await.unwrap();

    audit::end_sessions_for_user(&pool, "alice", "disconnect").await.unwrap();
    assert_eq!(open_rows(&pool, 10).await, 0);
    assert_eq!(open_rows(&pool, 11).await, 0);
    assert_eq!(open_rows(&pool, 12).await, 1);
}
