//! LC-186: remote-control session audit storage.

mod common;

use lets_chat::db::remote_control_audit as audit;
use sqlx::Row;

async fn open_rows(pool: &sqlx::SqlitePool, room_id: i64) -> i64 {
    sqlx::query(
        "SELECT COUNT(*) AS n FROM remote_control_sessions WHERE room_id = ? AND ended_at IS NULL",
    )
    .bind(room_id)
    .fetch_one(pool)
    .await
    .unwrap()
    .get::<i64, _>("n")
}

#[tokio::test]
async fn start_then_end_by_room() {
    let pool = common::chat_pool().await;
    audit::start_session(&pool, 7, "controller", "sharer")
        .await
        .unwrap();
    assert_eq!(open_rows(&pool, 7).await, 1);

    let closed = audit::end_session_by_room(&pool, 7, "revoked")
        .await
        .unwrap()
        .expect("a row was open to close");
    assert_eq!(closed.controller_id, "controller");
    assert_eq!(closed.sharer_id, "sharer");
    assert_eq!(open_rows(&pool, 7).await, 0);

    // A second close finds nothing open and closes nothing.
    assert!(audit::end_session_by_room(&pool, 7, "revoked")
        .await
        .unwrap()
        .is_none());

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
    audit::end_session_by_room(&pool, 1, "revoked")
        .await
        .unwrap();
    audit::start_session(&pool, 1, "c", "s").await.unwrap();
    assert_eq!(open_rows(&pool, 1).await, 1);
    let total: i64 =
        sqlx::query("SELECT COUNT(*) AS n FROM remote_control_sessions WHERE room_id = ?")
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
    audit::start_session(&pool, 10, "alice", "bob")
        .await
        .unwrap();
    audit::start_session(&pool, 11, "carol", "alice")
        .await
        .unwrap();
    // An unrelated session must survive.
    audit::start_session(&pool, 12, "bob", "carol")
        .await
        .unwrap();

    let mut closed = audit::end_sessions_for_user(&pool, "alice", "disconnect")
        .await
        .unwrap();
    closed.sort_by_key(|c| c.room_id);
    assert_eq!(closed.len(), 2);
    assert_eq!(closed[0].room_id, 10);
    assert_eq!(closed[0].controller_id, "alice");
    assert_eq!(closed[0].sharer_id, "bob");
    assert_eq!(closed[1].room_id, 11);
    assert_eq!(closed[1].controller_id, "carol");
    assert_eq!(closed[1].sharer_id, "alice");
    assert_eq!(open_rows(&pool, 10).await, 0);
    assert_eq!(open_rows(&pool, 11).await, 0);
    assert_eq!(open_rows(&pool, 12).await, 1);

    // A second call finds nothing left open for alice.
    assert!(audit::end_sessions_for_user(&pool, "alice", "disconnect")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn end_for_participant_requires_membership_and_is_idempotent() {
    let pool = common::chat_pool().await;
    audit::start_session(&pool, 20, "carol", "dave")
        .await
        .unwrap();

    // An outsider is not a party to the session: nothing closes.
    assert!(
        audit::end_session_for_participant(&pool, 20, "eve", "left_call")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(open_rows(&pool, 20).await, 1);

    // The controller is a party: closes, returns the row.
    let closed = audit::end_session_for_participant(&pool, 20, "carol", "left_call")
        .await
        .unwrap()
        .expect("carol was a party");
    assert_eq!(closed.controller_id, "carol");
    assert_eq!(closed.sharer_id, "dave");
    assert_eq!(open_rows(&pool, 20).await, 0);

    // Whoever observes the drop second finds nothing left to close.
    assert!(
        audit::end_session_for_participant(&pool, 20, "dave", "disconnect")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn end_by_sharer_ignores_the_controller() {
    let pool = common::chat_pool().await;
    audit::start_session(&pool, 30, "frank", "grace")
        .await
        .unwrap();

    // Frank is the controller, not the sharer: must not close the session.
    assert!(
        audit::end_session_by_sharer(&pool, 30, "frank", "share_ended")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(open_rows(&pool, 30).await, 1);

    // Grace is the sharer: closes.
    let closed = audit::end_session_by_sharer(&pool, 30, "grace", "share_ended")
        .await
        .unwrap()
        .expect("grace was the sharer");
    assert_eq!(closed.controller_id, "frank");
    assert_eq!(closed.sharer_id, "grace");
    assert_eq!(open_rows(&pool, 30).await, 0);
}
