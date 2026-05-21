//! Cascade verification tests for the per-room message retention path.
//!
//! Retention is the first hard-DELETE on `messages` to ever ship; every FK
//! ON DELETE action against `messages` has been theoretical until this
//! branch. This file proves the cascade fires as the schema claims for
//! every referencing row, before the retention sweep that exercises them
//! lands in the next commit. Each test seeds a `messages` row and a
//! referencing row, runs a direct DELETE against `messages`, and asserts
//! the referencing row is gone (CASCADE), nulled (SET NULL), or has the
//! right side-effect (the FTS DELETE trigger from migration 0045).
//!
//! Tests use the drift-immune common::chat_pool() helper (the new
//! prescribed pattern per CLAUDE.md test-maintenance).

mod common;

use sqlx::{Row, SqlitePool};

/// Open a fresh chat pool and turn FK enforcement on. sqlx 0.8 defaults
/// SqliteConnectOptions::foreign_keys to true, but explicitly setting the
/// PRAGMA here costs nothing and matches the belt-and-braces precedent in
/// db_scheduled.rs::room_delete_cascades_to_scheduled_rows.
async fn fresh_pool() -> SqlitePool {
    let pool = common::chat_pool().await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable FK enforcement");
    pool
}

async fn seed_room(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .expect("seed room")
        .last_insert_rowid()
}

async fn seed_message(pool: &SqlitePool, room_id: i64, body: &str) -> i64 {
    sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, 'u1', ?)")
        .bind(room_id)
        .bind(body)
        .execute(pool)
        .await
        .expect("seed message")
        .last_insert_rowid()
}

async fn hard_delete(pool: &SqlitePool, message_id: i64) {
    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(message_id)
        .execute(pool)
        .await
        .expect("hard delete");
}

async fn count_where_message_id(pool: &SqlitePool, table: &str, message_id: i64) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE message_id = ?");
    sqlx::query_scalar(&sql)
        .bind(message_id)
        .fetch_one(pool)
        .await
        .expect("count query")
}

// ----------------------------------------------------------------------
// CASCADE: referencing row vanishes when the message is hard-deleted.
// ----------------------------------------------------------------------

#[tokio::test]
async fn reaction_row_cascades_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "react-room").await;
    let target = seed_message(&pool, room, "react me").await;
    let control = seed_message(&pool, room, "spare").await;
    for mid in [target, control] {
        sqlx::query(
            "INSERT INTO message_reactions (message_id, user_id, emoji) \
             VALUES (?, 'u1', ':+1:')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(
        count_where_message_id(&pool, "message_reactions", target).await,
        0
    );
    assert_eq!(
        count_where_message_id(&pool, "message_reactions", control).await,
        1
    );
}

#[tokio::test]
async fn bookmark_row_cascades_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "bm-room").await;
    let target = seed_message(&pool, room, "saved").await;
    let control = seed_message(&pool, room, "spare").await;
    for mid in [target, control] {
        sqlx::query("INSERT INTO bookmarks (user_id, message_id) VALUES ('u1', ?)")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(count_where_message_id(&pool, "bookmarks", target).await, 0);
    assert_eq!(count_where_message_id(&pool, "bookmarks", control).await, 1);
}

#[tokio::test]
async fn pin_row_cascades_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "pin-room").await;
    let target = seed_message(&pool, room, "pinned").await;
    let control = seed_message(&pool, room, "spare").await;
    for mid in [target, control] {
        sqlx::query(
            "INSERT INTO pinned_messages (message_id, room_id, pinned_by) \
             VALUES (?, ?, 'u1')",
        )
        .bind(mid)
        .bind(room)
        .execute(&pool)
        .await
        .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(
        count_where_message_id(&pool, "pinned_messages", target).await,
        0
    );
    assert_eq!(
        count_where_message_id(&pool, "pinned_messages", control).await,
        1
    );
}

#[tokio::test]
async fn message_edit_rows_cascade_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "edit-room").await;
    let target = seed_message(&pool, room, "edited").await;
    let control = seed_message(&pool, room, "spare").await;
    for (mid, body) in [(target, "v1"), (target, "v2"), (control, "v1")] {
        sqlx::query(
            "INSERT INTO message_edits (message_id, previous_body, edited_at) \
             VALUES (?, ?, datetime('now'))",
        )
        .bind(mid)
        .bind(body)
        .execute(&pool)
        .await
        .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(
        count_where_message_id(&pool, "message_edits", target).await,
        0
    );
    assert_eq!(
        count_where_message_id(&pool, "message_edits", control).await,
        1
    );
}

#[tokio::test]
async fn mention_row_cascades_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "mention-room").await;
    let target = seed_message(&pool, room, "ping u2").await;
    let control = seed_message(&pool, room, "spare").await;
    for mid in [target, control] {
        sqlx::query(
            "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
             VALUES (?, ?, 'u2', 'u1')",
        )
        .bind(mid)
        .bind(room)
        .execute(&pool)
        .await
        .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(count_where_message_id(&pool, "mentions", target).await, 0);
    assert_eq!(count_where_message_id(&pool, "mentions", control).await, 1);
}

#[tokio::test]
async fn poll_rows_cascade_recursively_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "poll-room").await;
    let target = seed_message(&pool, room, "question").await;
    sqlx::query("INSERT INTO polls (message_id, question) VALUES (?, 'pick one')")
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    let opt1: i64 =
        sqlx::query("INSERT INTO poll_options (message_id, position, text) VALUES (?, 0, 'a')")
            .bind(target)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query("INSERT INTO poll_votes (option_id, user_id) VALUES (?, 'u1')")
        .bind(opt1)
        .execute(&pool)
        .await
        .unwrap();

    hard_delete(&pool, target).await;

    assert_eq!(count_where_message_id(&pool, "polls", target).await, 0);
    assert_eq!(
        count_where_message_id(&pool, "poll_options", target).await,
        0
    );
    let votes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poll_votes WHERE option_id = ?")
        .bind(opt1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(votes, 0, "poll_votes must cascade through poll_options");
}

#[tokio::test]
async fn reminder_row_cascades_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "reminder-room").await;
    let target = seed_message(&pool, room, "remind me").await;
    let control = seed_message(&pool, room, "spare").await;
    for mid in [target, control] {
        sqlx::query(
            "INSERT INTO reminders (user_id, message_id, remind_at) \
             VALUES ('u1', ?, '2099-01-01 00:00:00')",
        )
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();
    }

    hard_delete(&pool, target).await;

    assert_eq!(count_where_message_id(&pool, "reminders", target).await, 0);
    assert_eq!(count_where_message_id(&pool, "reminders", control).await, 1);
}

#[tokio::test]
async fn quarantine_row_cascades_on_message_hard_delete() {
    // Gap A regression: 0032 declared the FK without an action; 0044
    // rebuilds with ON DELETE CASCADE. If the rebuild ever regresses, this
    // test fails with SQLITE_CONSTRAINT_FOREIGNKEY on the DELETE.
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "quar-room").await;
    let target = seed_message(&pool, room, "spammy").await;
    sqlx::query(
        "INSERT INTO link_filter_quarantine (message_id, matched_pattern, matched_url) \
         VALUES (?, '*.tk', 'http://example.tk/x')",
    )
    .bind(target)
    .execute(&pool)
    .await
    .unwrap();

    hard_delete(&pool, target).await;

    assert_eq!(
        count_where_message_id(&pool, "link_filter_quarantine", target).await,
        0,
        "0044 must rebuild quarantine FK with ON DELETE CASCADE",
    );
}

#[tokio::test]
async fn thread_direct_child_cascades_when_root_hard_deleted() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "thread-room").await;
    let root = seed_message(&pool, room, "thread root").await;
    let child = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'reply', ?)",
    )
    .bind(room)
    .bind(root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, root).await;

    let surviving: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(child)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        surviving, 0,
        "thread child must cascade-delete with its root (messages.parent_id ON DELETE CASCADE)",
    );
}

#[tokio::test]
async fn thread_grandchild_cascades_recursively_when_root_hard_deleted() {
    // SQLite's ON DELETE CASCADE walks recursively. Verifies that deleting
    // a thread root removes the entire descendant tree, not just direct
    // children. Loosest-correct retention only deletes a root when the
    // newest reply is also past cutoff, so the recursive cascade is what
    // makes "delete the thread as a unit" work without a manual traversal.
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "deep-thread").await;
    let root = seed_message(&pool, room, "root").await;
    let child = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'child', ?)",
    )
    .bind(room)
    .bind(root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let grandchild = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u3', 'grandchild', ?)",
    )
    .bind(room)
    .bind(child)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, root).await;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id IN (?, ?, ?)")
        .bind(root)
        .bind(child)
        .bind(grandchild)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0, "root + child + grandchild must all be gone");
}

#[tokio::test]
async fn unrelated_thread_unaffected_by_sibling_root_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "siblings").await;
    let root_a = seed_message(&pool, room, "root A").await;
    let child_a = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'child A', ?)",
    )
    .bind(room)
    .bind(root_a)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let root_b = seed_message(&pool, room, "root B").await;
    let child_b = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'child B', ?)",
    )
    .bind(room)
    .bind(root_b)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, root_a).await;

    let a_survivors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id IN (?, ?)")
        .bind(root_a)
        .bind(child_a)
        .fetch_one(&pool)
        .await
        .unwrap();
    let b_survivors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id IN (?, ?)")
        .bind(root_b)
        .bind(child_b)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(a_survivors, 0);
    assert_eq!(b_survivors, 2, "unrelated thread must not be touched");
}

// ----------------------------------------------------------------------
// SET NULL: referencing row survives with the FK column nulled.
// ----------------------------------------------------------------------

#[tokio::test]
async fn quote_id_nullifies_when_quoted_message_hard_deleted() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "quote-room").await;
    let quoted = seed_message(&pool, room, "original").await;
    let quoter = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, quote_id) \
         VALUES (?, 'u2', 'reply quoting', ?)",
    )
    .bind(room)
    .bind(quoted)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, quoted).await;

    let quote_id: Option<i64> = sqlx::query("SELECT quote_id FROM messages WHERE id = ?")
        .bind(quoter)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert!(
        quote_id.is_none(),
        "quote_id must be SET NULL when the quoted message is hard-deleted",
    );
}

#[tokio::test]
async fn file_upload_message_id_nullifies_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "upload-room").await;
    let target = seed_message(&pool, room, "with attachment").await;
    let upload = sqlx::query(
        "INSERT INTO file_uploads \
         (uploader_id, message_id, filename, mime_type, size_bytes, storage_path) \
         VALUES ('u1', ?, 'pic.png', 'image/png', 10, '/tmp/x')",
    )
    .bind(target)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, target).await;

    let row = sqlx::query("SELECT message_id FROM file_uploads WHERE id = ?")
        .bind(upload)
        .fetch_one(&pool)
        .await
        .unwrap();
    let message_id: Option<i64> = row.get(0);
    assert!(
        message_id.is_none(),
        "file_uploads.message_id must SET NULL so the orphan sweeper claims it on its own cadence",
    );
}

#[tokio::test]
async fn scheduled_parent_and_quote_nullify_on_message_hard_delete() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "sched-room").await;
    let parent_msg = seed_message(&pool, room, "thread root").await;
    let quoted_msg = seed_message(&pool, room, "to be quoted").await;
    let sched = sqlx::query(
        "INSERT INTO scheduled_messages \
         (room_id, user_id, body, scheduled_for, parent_id, quote_id) \
         VALUES (?, 'u1', 'pending', '2099-01-01 00:00:00', ?, ?)",
    )
    .bind(room)
    .bind(parent_msg)
    .bind(quoted_msg)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, parent_msg).await;
    hard_delete(&pool, quoted_msg).await;

    let row = sqlx::query("SELECT parent_id, quote_id FROM scheduled_messages WHERE id = ?")
        .bind(sched)
        .fetch_one(&pool)
        .await
        .unwrap();
    let parent_id: Option<i64> = row.get(0);
    let quote_id: Option<i64> = row.get(1);
    assert!(
        parent_id.is_none(),
        "scheduled_messages.parent_id must SET NULL"
    );
    assert!(
        quote_id.is_none(),
        "scheduled_messages.quote_id must SET NULL"
    );
}

// ----------------------------------------------------------------------
// Trigger: messages_fts entry gone after DELETE (migration 0045).
// ----------------------------------------------------------------------

#[tokio::test]
async fn fts_entry_removed_when_message_hard_deleted() {
    // 0008 wired triggers for INSERT, UPDATE-of-body, UPDATE-of-deleted_at,
    // but NOT for DELETE. 0045 adds the missing AFTER DELETE trigger so the
    // retention sweep does not leave phantom search hits.
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "fts-room").await;
    // Plain alphabetic token: FTS5's MATCH parser treats hyphens and bare
    // digits as query operators / column references, so an embedded "-"
    // or numeric segment in the search term explodes at parse time.
    let token = "xenonberryunique";
    let target = seed_message(&pool, room, token).await;

    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?")
            .bind(token)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, 1, "FTS insert trigger should have indexed the row");

    hard_delete(&pool, target).await;

    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?")
            .bind(token)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, 0, "0045 must remove the FTS row on hard-delete");
}

// ----------------------------------------------------------------------
// Integration: a single message with every kind of referencing row.
// Proves the cascade composes when the retention sweep hits a "rich"
// message in production. Catches FK-interaction surprises (e.g., one row
// blocking another's cleanup) that the isolated tests above might miss.
// ----------------------------------------------------------------------

#[tokio::test]
async fn rich_message_with_all_references_hard_deletes_cleanly() {
    let pool = fresh_pool().await;
    let room = seed_room(&pool, "rich-room").await;
    let m = seed_message(&pool, room, "rich one").await;

    // CASCADE rows
    sqlx::query(
        "INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, 'u1', ':+1:')",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO bookmarks (user_id, message_id) VALUES ('u1', ?)")
        .bind(m)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO pinned_messages (message_id, room_id, pinned_by) VALUES (?, ?, 'u1')")
        .bind(m)
        .bind(room)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO message_edits (message_id, previous_body, edited_at) \
         VALUES (?, 'v1', datetime('now'))",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, 'u2', 'u1')",
    )
    .bind(m)
    .bind(room)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO polls (message_id, question) VALUES (?, 'q')")
        .bind(m)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reminders (user_id, message_id, remind_at) \
         VALUES ('u1', ?, '2099-01-01 00:00:00')",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO link_filter_quarantine (message_id, matched_pattern, matched_url) \
         VALUES (?, '*.tk', 'http://x.tk/')",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap();
    // SET NULL rows
    let upload = sqlx::query(
        "INSERT INTO file_uploads \
         (uploader_id, message_id, filename, mime_type, size_bytes, storage_path) \
         VALUES ('u1', ?, 'a.png', 'image/png', 10, '/tmp/a')",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let sched = sqlx::query(
        "INSERT INTO scheduled_messages \
         (room_id, user_id, body, scheduled_for, parent_id, quote_id) \
         VALUES (?, 'u1', 's', '2099-01-01 00:00:00', ?, ?)",
    )
    .bind(room)
    .bind(m)
    .bind(m)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let quoter = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, quote_id) \
         VALUES (?, 'u2', 'quote', ?)",
    )
    .bind(room)
    .bind(m)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    hard_delete(&pool, m).await;

    // Every CASCADE row gone.
    for table in [
        "message_reactions",
        "bookmarks",
        "pinned_messages",
        "message_edits",
        "mentions",
        "polls",
        "reminders",
        "link_filter_quarantine",
    ] {
        assert_eq!(
            count_where_message_id(&pool, table, m).await,
            0,
            "{table} must cascade-delete",
        );
    }

    // SET NULL rows survive with nulled FK.
    let upload_row = sqlx::query("SELECT message_id FROM file_uploads WHERE id = ?")
        .bind(upload)
        .fetch_one(&pool)
        .await
        .unwrap();
    let upload_msg: Option<i64> = upload_row.get(0);
    assert!(upload_msg.is_none());

    let sched_row = sqlx::query("SELECT parent_id, quote_id FROM scheduled_messages WHERE id = ?")
        .bind(sched)
        .fetch_one(&pool)
        .await
        .unwrap();
    let sched_parent: Option<i64> = sched_row.get(0);
    let sched_quote: Option<i64> = sched_row.get(1);
    assert!(sched_parent.is_none());
    assert!(sched_quote.is_none());

    let quoter_row = sqlx::query("SELECT quote_id FROM messages WHERE id = ?")
        .bind(quoter)
        .fetch_one(&pool)
        .await
        .unwrap();
    let quoter_quote: Option<i64> = quoter_row.get(0);
    assert!(quoter_quote.is_none());

    // FTS entry gone via the 0045 trigger.
    let fts_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'rich'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fts_hits, 0);
}
