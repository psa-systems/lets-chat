use sqlx::{Row, SqlitePool};

mod common;

async fn setup_pool() -> SqlitePool {
    common::chat_pool().await
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
async fn test_search_home_excludes_rooms_outside_callers_enclaves() {
    // u-a is not a member of any enclave. Their home search must return only
    // the DM hit and skip the public room in General.
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
    assert_eq!(
        results.len(),
        1,
        "non-member must not see rooms in enclaves they are not in"
    );
    assert_eq!(results[0].room_name, "@u-b");
}

#[tokio::test]
async fn test_search_home_includes_public_rooms_in_callers_enclaves() {
    // After joining General u-a's home search returns both the DM hit and
    // the public room hit.
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    lets_chat::db::enclave::add_member(
        &pool,
        g,
        "u-a",
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let public_room =
        lets_chat::db::chat::create_room(&pool, "rooms", None, "public", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, public_room, "u-other", "find me")
        .await
        .unwrap();
    let dm = lets_chat::db::chat::create_dm_room(&pool, "@u-b", "u-a", "u-b")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, dm.id, "u-a", "find me")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("find").unwrap();
    let mut results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, None, true, "u-a", false)
            .await
            .unwrap();
    results.sort_by(|a, b| a.room_name.cmp(&b.room_name));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].room_name, "@u-b");
    assert_eq!(results[1].room_name, "rooms");
}

#[tokio::test]
async fn test_search_home_excludes_private_rooms_caller_is_not_in() {
    // u-a is in the enclave but not in the private room; the private room hit
    // must not appear.
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    lets_chat::db::enclave::add_member(
        &pool,
        g,
        "u-a",
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let private_room =
        lets_chat::db::chat::create_room(&pool, "vault", None, "private", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "u-other", "secret find")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("secret").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, None, true, "u-a", false)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_search_home_includes_private_rooms_caller_is_member_of() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    lets_chat::db::enclave::add_member(
        &pool,
        g,
        "u-a",
        lets_chat::models::enclave::EnclaveRole::Member,
    )
    .await
    .unwrap();
    let private_room =
        lets_chat::db::chat::create_room(&pool, "team", None, "private", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::add_room_member(&pool, private_room, "u-a")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, private_room, "u-a", "private hits")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("private").unwrap();
    let results = lets_chat::db::chat::search_messages(&pool, &fts, None, None, true, "u-a", false)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].room_name, "team");
}

#[tokio::test]
async fn test_search_home_admin_sees_all_non_dm_rooms() {
    // A site admin's home search returns every non-DM room hit even without
    // explicit enclave membership; their DM scope is still gated on
    // room_members so they cannot quietly read foreign DMs.
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let public_room =
        lets_chat::db::chat::create_room(&pool, "global", None, "public", None, Some(g))
            .await
            .unwrap();
    lets_chat::db::chat::insert_message(&pool, public_room, "u-other", "find me")
        .await
        .unwrap();
    let dm = lets_chat::db::chat::create_dm_room(&pool, "@u-b", "u-c", "u-b")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, dm.id, "u-c", "find me")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("find").unwrap();
    let results =
        lets_chat::db::chat::search_messages(&pool, &fts, None, None, true, "admin-1", true)
            .await
            .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].room_name, "global");
}

// ----------------------------------------------------------------------
// LC-280: operator filters (from: / before: / after:) via SearchFilters.
// ----------------------------------------------------------------------

#[tokio::test]
async fn search_filters_by_author() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room = lets_chat::db::chat::create_room(&pool, "by-author", None, "public", None, Some(g))
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room, "alice", "shared term")
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(&pool, room, "bob", "shared term")
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("shared").unwrap();
    let filters = lets_chat::db::chat::SearchFilters {
        author_id: Some("alice".into()),
        before: None,
        after: None,
        ..Default::default()
    };
    let res = lets_chat::db::chat::search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "alice",
        false,
        &filters,
    )
    .await
    .unwrap();
    assert_eq!(res.len(), 1, "author filter keeps only alice's hit");
    assert_eq!(res[0].user_id, "alice");
}

#[tokio::test]
async fn search_filters_by_date_range() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room = lets_chat::db::chat::create_room(&pool, "by-date", None, "public", None, Some(g))
        .await
        .unwrap();
    let old = lets_chat::db::chat::insert_message(&pool, room, "user-1", "dated term")
        .await
        .unwrap();
    let new = lets_chat::db::chat::insert_message(&pool, room, "user-1", "dated term")
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET created_at = '2020-01-01 00:00:00' WHERE id = ?")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET created_at = '2024-01-01 00:00:00' WHERE id = ?")
        .bind(new)
        .execute(&pool)
        .await
        .unwrap();

    let fts = lets_chat::db::chat::sanitize_fts_query("dated").unwrap();

    // before:2022 keeps only the 2020 message.
    let before = lets_chat::db::chat::SearchFilters {
        author_id: None,
        before: Some("2022-01-01".into()),
        after: None,
        ..Default::default()
    };
    let res = lets_chat::db::chat::search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &before,
    )
    .await
    .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].message_id, old);

    // after:2022 keeps only the 2024 message.
    let after = lets_chat::db::chat::SearchFilters {
        author_id: None,
        before: None,
        after: Some("2022-01-01".into()),
        ..Default::default()
    };
    let res = lets_chat::db::chat::search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &after,
    )
    .await
    .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].message_id, new);
}

// LC-530: has:file / has:link / in:thread search refinements.
#[tokio::test]
async fn test_search_filters_has_file_link_thread() {
    use lets_chat::db::chat::{
        create_room, insert_message, sanitize_fts_query, search_messages_filtered, SearchFilters,
    };
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room = create_room(&pool, "search-filters", None, "public", None, Some(g))
        .await
        .unwrap();

    // Four messages all matching the term "report".
    let _plain = insert_message(&pool, room, "user-1", "report plain")
        .await
        .unwrap();
    let file_msg = insert_message(&pool, room, "user-1", "report with attachment")
        .await
        .unwrap();
    let link_msg = insert_message(&pool, room, "user-1", "report see https://example.com")
        .await
        .unwrap();
    // A thread reply (parent_id set) via raw SQL; the FTS insert trigger still fires.
    let reply = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) VALUES (?, 'user-1', 'report reply', ?)",
    )
    .bind(room)
    .bind(_plain)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    // Attach a file to file_msg.
    sqlx::query(
        "INSERT INTO file_uploads (uploader_id, message_id, filename, mime_type, size_bytes, storage_path) \
         VALUES ('user-1', ?, 'a.png', 'image/png', 1, 'x')",
    )
    .bind(file_msg)
    .execute(&pool)
    .await
    .unwrap();

    let fts = sanitize_fts_query("report").unwrap();

    // No refinements: all four match.
    let all = search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &SearchFilters::default(),
    )
    .await
    .unwrap();
    assert_eq!(all.len(), 4);

    // has:file -> only the message with an attachment.
    let r = search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &SearchFilters {
            has_file: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].message_id, file_msg);

    // has:link -> only the message with an http(s) URL.
    let r = search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &SearchFilters {
            has_link: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].message_id, link_msg);

    // in:thread -> only the reply (parent_id set).
    let r = search_messages_filtered(
        &pool,
        &fts,
        None,
        Some(g),
        false,
        "user-1",
        false,
        &SearchFilters {
            in_thread: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].message_id, reply);
}

// LC-676: the /ask RAG retrieval must find room context for a natural-language
// question. The old AND semantics ("who" AND "is" AND "david") missed a room
// full of David; `fts_query_any` drops stopwords + ORs the rest so it matches.

#[test]
fn fts_query_any_drops_stopwords_and_ors_the_rest() {
    let q = lets_chat::db::chat::fts_query_any("who is david?").unwrap();
    assert_eq!(q, "\"david\"");
    let q2 = lets_chat::db::chat::fts_query_any("what did we decide about the launch").unwrap();
    assert_eq!(q2, "\"decide\" OR \"launch\"");
    // An all-stopword question still yields a usable query rather than None.
    assert!(lets_chat::db::chat::fts_query_any("who is it").is_some());
    assert!(lets_chat::db::chat::fts_query_any("   ").is_none());
}

#[tokio::test]
async fn ask_retrieval_finds_room_context_the_and_query_missed() {
    let pool = setup_pool().await;
    let g = general_id(&pool).await;
    let room_id = lets_chat::db::chat::create_room(&pool, "ask-ctx", None, "public", None, Some(g))
        .await
        .unwrap();
    lets_chat::db::chat::insert_message(
        &pool,
        room_id,
        "user-1",
        "David really stands out at Simcha Health, leading clinical trials.",
    )
    .await
    .unwrap();
    lets_chat::db::chat::insert_message(&pool, room_id, "user-2", "lunch plans for friday")
        .await
        .unwrap();

    // The old AND query for the natural-language question matches nothing.
    let and_q = lets_chat::db::chat::sanitize_fts_query("who is david?").unwrap();
    let and_hits = lets_chat::db::chat::fts_room_context(&pool, room_id, &and_q, 12)
        .await
        .unwrap();
    assert!(
        and_hits.is_empty(),
        "AND semantics dead-end on a question: {and_hits:?}"
    );

    // The RAG OR query retrieves the David message (and not the lunch one).
    let any_q = lets_chat::db::chat::fts_query_any("who is david?").unwrap();
    let hits = lets_chat::db::chat::fts_room_context(&pool, room_id, &any_q, 12)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "retrieves the David context: {hits:?}");
    assert!(hits[0].1.contains("David"));
}
