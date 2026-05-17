use sqlx::SqlitePool;

mod common;

async fn setup_pool() -> SqlitePool {
    common::chat_pool().await
}

#[tokio::test]
async fn test_edit_message_updates_body_and_sets_edited_at() {
    let pool = setup_pool().await;

    let room_id = lets_chat::db::chat::create_room(&pool, "test-edit", None, "public", None, None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "original body")
        .await
        .unwrap();

    // Before edit: edited_at should be None
    let msg = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.body, "original body");
    assert!(msg.edited_at.is_none());

    // Edit the message
    let edited_at = lets_chat::db::chat::update_message_body(&pool, msg_id, "edited body")
        .await
        .unwrap();
    assert!(!edited_at.is_empty());

    // After edit: body and edited_at should be updated
    let msg = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.body, "edited body");
    assert_eq!(msg.edited_at, Some(edited_at));
}

#[tokio::test]
async fn test_get_message_returns_none_for_soft_deleted() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-softdelete", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello")
        .await
        .unwrap();

    // Soft-delete the message
    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?",
    )
    .bind(msg_id)
    .execute(&pool)
    .await
    .unwrap();

    // get_message must return None for soft-deleted messages
    let result = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_list_messages_includes_edited_at() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-list-edited", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "first message")
        .await
        .unwrap();

    // Before edit: edited_at is None
    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].edited_at.is_none());
    assert_eq!(messages[0].body, "first message");

    // Edit it
    lets_chat::db::chat::update_message_body(&pool, msg_id, "updated message")
        .await
        .unwrap();

    // After edit: body updated, edited_at is Some
    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].edited_at.is_some());
    assert_eq!(messages[0].body, "updated message");
}

#[tokio::test]
async fn test_first_edit_inserts_one_message_edits_row_with_prior_body() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-history-1", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "version zero")
        .await
        .unwrap();

    let edited_at = lets_chat::db::chat::update_message_body(&pool, msg_id, "version one")
        .await
        .unwrap();

    let edits = lets_chat::db::chat::list_message_edits(&pool, msg_id)
        .await
        .unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].message_id, msg_id);
    assert_eq!(edits[0].previous_body, "version zero");
    assert_eq!(edits[0].edited_at, edited_at);

    let m = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.body, "version one");
    assert_eq!(m.edited_at.as_deref(), Some(edited_at.as_str()));
}

#[tokio::test]
async fn test_two_edits_produce_two_message_edits_rows_chronological() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-history-2", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "v0")
        .await
        .unwrap();
    let t1 = lets_chat::db::chat::update_message_body(&pool, msg_id, "v1")
        .await
        .unwrap();
    let t2 = lets_chat::db::chat::update_message_body(&pool, msg_id, "v2")
        .await
        .unwrap();

    let edits = lets_chat::db::chat::list_message_edits(&pool, msg_id)
        .await
        .unwrap();
    assert_eq!(edits.len(), 2);
    // First row archives the body the first edit displaced.
    assert_eq!(edits[0].previous_body, "v0");
    assert_eq!(edits[0].edited_at, t1);
    // Second row archives the body the second edit displaced.
    assert_eq!(edits[1].previous_body, "v1");
    assert_eq!(edits[1].edited_at, t2);
    // Insertion order is preserved by the (edited_at ASC, id ASC) sort even
    // when t1 == t2 at second resolution (chrono's "%Y-%m-%d %H:%M:%S"
    // format has no sub-second component, so back-to-back tokio-test edits
    // routinely collide on the timestamp).
    assert!(edits[0].id < edits[1].id);

    let m = lets_chat::db::chat::get_message(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.body, "v2");
}

#[tokio::test]
async fn test_list_message_edits_empty_for_unedited_message() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-history-3", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "only version")
        .await
        .unwrap();

    let edits = lets_chat::db::chat::list_message_edits(&pool, msg_id)
        .await
        .unwrap();
    assert!(edits.is_empty());
}

#[tokio::test]
async fn test_soft_deleted_messages_not_returned_by_list_messages() {
    let pool = setup_pool().await;

    let room_id =
        lets_chat::db::chat::create_room(&pool, "test-deleted-list", None, "public", None, None)
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "will be deleted")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "stays alive")
        .await
        .unwrap();

    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?",
    )
    .bind(msg_id)
    .execute(&pool)
    .await
    .unwrap();

    let messages = lets_chat::db::chat::list_messages(&pool, room_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, "stays alive");
}
