use sqlx::{Row, SqlitePool};

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
        include_str!("../migrations/chat/0009_enclaves.sql"),
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

async fn general_id(pool: &SqlitePool) -> i64 {
    sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(pool)
        .await
        .unwrap()
        .get("id")
}

#[tokio::test]
async fn test_search_finds_matching_message() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "search-find", None, "public", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "hello world")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("hello").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "user-1", false)
            .await
            .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].body, "hello world");
    assert_eq!(results[0].room_name, "search-find");
}

#[tokio::test]
async fn test_search_does_not_return_soft_deleted() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "search-deleted", None, "public", None, Some(g))
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "secret message")
        .await
        .unwrap();

    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'user-1' WHERE id = ?",
    )
    .bind(msg_id)
    .execute(&pool)
    .await
    .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("secret").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "user-1", false)
            .await
            .unwrap();

    assert!(
        results.is_empty(),
        "deleted messages must not appear in search"
    );
}

#[tokio::test]
async fn test_search_private_room_excluded_for_non_member() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let private_room =
        lets_chat::db::chat::create_room(&pool, "secret", None, "private", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "classified info")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("classified").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "user-2", false)
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
    let g = general_id(&pool).await;
    let private_room =
        lets_chat::db::chat::create_room(&pool, "vip", None, "private", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::add_room_member(&pool, private_room, "user-1")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "members only content")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("members").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "user-1", false)
            .await
            .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].room_name, "vip");
}

#[tokio::test]
async fn test_search_fts_special_chars_do_not_panic() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "search-special", None, "public", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "normal message")
        .await
        .unwrap();

    for raw in &["*", "()", "\"quoted\"", "a*b+c", "--drop", ""] {
        let maybe_fts = lets_chat::db::chat::sanitize_fts_query(raw);
        if let Some(fts) = maybe_fts {
            let result = lets_chat::db::chat::search_messages(
                &pool,
                &fts,
                None,
                Some(g),
                false,
                "user-1",
                false,
            )
            .await;
            assert!(result.is_ok(), "query {raw:?} caused an error: {result:?}");
        }
    }
}

#[tokio::test]
async fn test_search_edited_body_is_reindexed() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room_id =
        lets_chat::db::chat::create_room(&pool, "search-edit", None, "public", None, Some(g))
            .await
            .unwrap();
    let msg_id = lets_chat::db::chat::insert_message(&pool, room_id, "user-1", "original text")
        .await
        .unwrap();

    lets_chat::db::chat::update_message_body(&pool, msg_id, "completely revised content")
        .await
        .unwrap();

    let fts_old = lets_chat::db::chat::sanitize_fts_query("original").unwrap();
    let old_results = lets_chat::db::chat::search_messages(
        &pool,
        &fts_old,
        None,
        Some(g),
        false,
        "user-1",
        false,
    )
    .await
    .unwrap();
    assert!(old_results.is_empty(), "old term must not match after edit");

    let fts_new = lets_chat::db::chat::sanitize_fts_query("revised").unwrap();
    let new_results = lets_chat::db::chat::search_messages(
        &pool,
        &fts_new,
        None,
        Some(g),
        false,
        "user-1",
        false,
    )
    .await
    .unwrap();
    assert_eq!(new_results.len(), 1);
    assert_eq!(new_results[0].body, "completely revised content");
}

#[tokio::test]
async fn test_admin_can_search_private_rooms() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let private_room =
        lets_chat::db::chat::create_room(&pool, "admin-only", None, "private", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "user-1", "top secret")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("secret").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "admin-1", true)
            .await
            .unwrap();

    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_scoped_to_enclave_excludes_other_enclaves() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let other = lets_chat::db::enclave::create_enclave(&pool, "Other", None, "u-owner")
        .await
        .unwrap();
    let r_in_general =
        lets_chat::db::chat::create_room(&pool, "general-room", None, "public", None, Some(g))
            .await
            .unwrap();
    let r_in_other =
        lets_chat::db::chat::create_room(&pool, "other-room", None, "public", None, Some(other))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, r_in_general, "u-owner", "shared word here")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, r_in_other, "u-owner", "shared word here")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("shared").unwrap();
    let general_results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, Some(g), false, "u-owner", false)
            .await
            .unwrap();
    assert_eq!(general_results.len(), 1);
    assert_eq!(general_results[0].room_name, "general-room");
}

#[tokio::test]
async fn test_search_home_returns_only_dms() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let public_room = lets_chat::db::chat::create_room(&pool, "p", None, "public", None, Some(g))
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, public_room, "u-a", "find me")
        .await
        .unwrap();

    let dm = lets_chat::db::chat::create_dm_room(&pool, "@u-b", "u-a", "u-b")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, dm.id, "u-a", "find me")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("find").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, None, true, "u-a", false)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "Home search must return DM message only");
}
