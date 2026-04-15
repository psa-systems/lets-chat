use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory pool");

    for (i, sql) in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
    ]
    .iter()
    .enumerate()
    {
        sqlx::raw_sql(sql)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("migration {} failed: {e}", i + 1));
    }

    pool
}

#[tokio::test]
async fn test_search_finds_matching_message() {
    let pool = setup_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "search-find", None, "public", None)
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello world")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("hello").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, "user-1", false)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].body, "hello world");
    assert_eq!(results[0].room_name, "search-find");
}

#[tokio::test]
async fn test_search_does_not_return_soft_deleted() {
    let pool = setup_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "search-deleted", None, "public", None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "secret message")
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

    let fts = lets_chat::db::chat::sanitize_fts_query("secret").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, "user-1", false)
        .await
        .unwrap();

    assert!(results.is_empty(), "deleted messages must not appear in search");
}

#[tokio::test]
async fn test_search_private_room_excluded_for_non_member() {
    let pool = setup_pool().await;
    let private_room = lets_chat::db::chat::create_room(&pool, "secret", None, "private", None)
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "classified info")
        .await
        .unwrap();

    // user-2 is NOT a member of the private room
    let fts = lets_chat::db::chat::sanitize_fts_query("classified").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, "user-2", false)
        .await
        .unwrap();

    assert!(
        results.is_empty(),
        "non-member must not see private room messages"
    );
}

#[tokio::test]
async fn test_search_private_room_visible_to_member() {
    let pool = setup_pool().await;
    let private_room = lets_chat::db::chat::create_room(&pool, "vip", None, "private", None)
        .await
        .unwrap();
    lets_chat::db::chat::add_room_member(&pool, private_room, "user-1")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "members only content")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("members").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, "user-1", false)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].room_name, "vip");
}

#[tokio::test]
async fn test_search_fts_special_chars_do_not_panic() {
    let pool = setup_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "search-special", None, "public", None)
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "normal message")
        .await
        .unwrap();

    // These should all sanitize cleanly without panicking
    for raw in &["*", "()", "\"quoted\"", "a*b+c", "--drop", ""] {
        let maybe_fts = lets_chat::db::chat::sanitize_fts_query(raw);
        if let Some(fts) = maybe_fts {
            let result =
                lets_chat::db::chat::search_messages(&pool, &fts, None, "user-1", false).await;
            assert!(result.is_ok(), "query {raw:?} caused an error: {result:?}");
        }
        // Empty sanitized query returns None — that's correct, no DB call needed
    }
}

#[tokio::test]
async fn test_search_edited_body_is_reindexed() {
    let pool = setup_pool().await;
    let room_id = lets_chat::db::chat::create_room(&pool, "search-edit", None, "public", None)
        .await
        .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "original text")
        .await
        .unwrap();

    // Edit the message — the update trigger should reindex
    lets_chat::db::chat::update_message_body(&pool, msg_id, "completely revised content")
        .await
        .unwrap();

    // Old term should no longer match
    let fts_old = lets_chat::db::chat::sanitize_fts_query("original").unwrap();
    let old_results =
        lets_chat::db::chat::search_messages(&pool, &fts_old, None, "user-1", false)
            .await
            .unwrap();
    assert!(old_results.is_empty(), "old term must not match after edit");

    // New term should match
    let fts_new = lets_chat::db::chat::sanitize_fts_query("revised").unwrap();
    let new_results =
        lets_chat::db::chat::search_messages(&pool, &fts_new, None, "user-1", false)
            .await
            .unwrap();
    assert_eq!(new_results.len(), 1);
    assert_eq!(new_results[0].body, "completely revised content");
}

#[tokio::test]
async fn test_admin_can_search_private_rooms() {
    let pool = setup_pool().await;
    let private_room = lets_chat::db::chat::create_room(&pool, "admin-only", None, "private", None)
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "top secret")
        .await
        .unwrap();

    // Admin (is_admin=true) should see all non-DM rooms
    let fts = lets_chat::db::chat::sanitize_fts_query("secret").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, "admin-1", true)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
}
