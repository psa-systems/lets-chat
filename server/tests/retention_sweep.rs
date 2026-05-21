//! Behavior tests for `crate::retention::sweep`.
//!
//! Verifies the loose-correct sweep predicate, the LIMIT-bounded
//! per-tick delete, the rooms.room_type != 'dm' exclusion, the
//! settled-decision inclusions (soft-deleted / quarantined / system /
//! pinned messages all hard-delete past cutoff), and the
//! count_candidates_for_room == sweep_once invariant that keeps the
//! room-settings preview from drifting away from the destructive path.
//!
//! Tests call `sweep_once` directly to exercise the destructive body
//! without touching the process-wide `LETS_CHAT_RETENTION_SWEEP_ENABLED`
//! env var. The flag check is one if-statement in `run_retention_sweep`;
//! it stays code-review territory, not test territory, because env-var
//! state cannot be safely manipulated by parallel tests within one
//! binary.

mod common;

use lets_chat::retention::sweep::{count_candidates_for_room, sweep_once, SweepStats, SWEEP_LIMIT};
use sqlx::SqlitePool;

async fn fresh_pool() -> SqlitePool {
    let pool = common::chat_pool().await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable FK enforcement");
    pool
}

async fn make_room(pool: &SqlitePool, name: &str, retention_days: Option<i64>) -> i64 {
    let room_id: i64 = sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'public')")
        .bind(name)
        .execute(pool)
        .await
        .expect("insert room")
        .last_insert_rowid();
    if let Some(days) = retention_days {
        sqlx::query("UPDATE rooms SET retention_days = ? WHERE id = ?")
            .bind(days)
            .bind(room_id)
            .execute(pool)
            .await
            .unwrap();
    }
    room_id
}

async fn make_dm_room(pool: &SqlitePool, name: &str, retention_days: Option<i64>) -> i64 {
    let room_id: i64 = sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'dm')")
        .bind(name)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    if let Some(days) = retention_days {
        sqlx::query("UPDATE rooms SET retention_days = ? WHERE id = ?")
            .bind(days)
            .bind(room_id)
            .execute(pool)
            .await
            .unwrap();
    }
    room_id
}

/// Insert a message and force its `created_at` to `age_days` ago. The
/// retention sweep keys off `created_at`, so backdating is how tests
/// produce "past cutoff" without waiting in real time.
async fn make_message(pool: &SqlitePool, room: i64, body: &str, age_days: i64) -> i64 {
    let id: i64 = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, 'u1', ?)")
        .bind(room)
        .bind(body)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    if age_days != 0 {
        sqlx::query("UPDATE messages SET created_at = datetime('now', ?) WHERE id = ?")
            .bind(format!("-{age_days} days"))
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn count_messages(pool: &SqlitePool, room: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn message_exists(pool: &SqlitePool, id: i64) -> bool {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    n == 1
}

// ----------------------------------------------------------------------
// Sweep behavior: skipping rooms that should not be touched.
// ----------------------------------------------------------------------

#[tokio::test]
async fn empty_pool_sweep_returns_default_stats() {
    let pool = fresh_pool().await;
    let stats = sweep_once(&pool).await.unwrap();
    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(stats.rooms_touched, 0);
    assert!(!stats.flag_disabled);
}

#[tokio::test]
async fn room_with_no_retention_configured_is_skipped() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "no-retention", None).await;
    make_message(&pool, room, "old", 365).await;

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(count_messages(&pool, room).await, 1);
}

#[tokio::test]
async fn dm_room_with_retention_set_is_skipped_via_predicate() {
    // Defense-in-depth: the room-settings UI in a later commit will
    // refuse to set retention on a DM, but the sweep predicate also
    // excludes room_type='dm' so a value that arrives by any other path
    // (manual SQL, future schema change) still cannot delete DM messages.
    let pool = fresh_pool().await;
    let dm = make_dm_room(&pool, "dm-r", Some(30)).await;
    make_message(&pool, dm, "stale dm", 60).await;

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(count_messages(&pool, dm).await, 1);
}

// ----------------------------------------------------------------------
// Sweep behavior: the cutoff boundary.
// ----------------------------------------------------------------------

#[tokio::test]
async fn message_past_cutoff_is_deleted() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    make_message(&pool, room, "stale", 31).await;

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 1);
    assert_eq!(stats.rooms_touched, 1);
    assert_eq!(count_messages(&pool, room).await, 0);
}

#[tokio::test]
async fn message_before_cutoff_survives() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    make_message(&pool, room, "recent", 29).await;

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(count_messages(&pool, room).await, 1);
}

#[tokio::test]
async fn message_exactly_at_cutoff_survives_strict_less_than() {
    // The predicate uses `created_at < cutoff` (strict). A message whose
    // age equals the retention window survives one tick and dies on the
    // next. Documenting via test rather than relying on implementation
    // inspection: a future refactor that changes `<` to `<=` would flip
    // this assertion and is something a reviewer should consciously sign
    // off on.
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    make_message(&pool, room, "boundary", 30).await;

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(count_messages(&pool, room).await, 1);
}

// ----------------------------------------------------------------------
// Sweep behavior: LIMIT-bounded per-tick, multi-tick drain.
// ----------------------------------------------------------------------

#[tokio::test]
async fn limit_bounds_per_tick_delete() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "big", Some(30)).await;
    let extra = 5;
    let total = SWEEP_LIMIT + extra;
    for i in 0..total {
        make_message(&pool, room, &format!("stale-{i}"), 31).await;
    }

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(
        stats.messages_deleted as i64, SWEEP_LIMIT,
        "single tick must not exceed SWEEP_LIMIT",
    );
    assert_eq!(count_messages(&pool, room).await, extra);
}

#[tokio::test]
async fn multiple_ticks_drain_backlog() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "drain", Some(30)).await;
    let total = SWEEP_LIMIT + 7;
    for i in 0..total {
        make_message(&pool, room, &format!("stale-{i}"), 31).await;
    }

    sweep_once(&pool).await.unwrap();
    sweep_once(&pool).await.unwrap();

    assert_eq!(count_messages(&pool, room).await, 0);
}

// ----------------------------------------------------------------------
// Sweep behavior: threads (loose-correct, sweep-by-newest-reply).
// ----------------------------------------------------------------------

#[tokio::test]
async fn thread_with_recent_reply_preserves_root() {
    // Loose-correct semantic in action: a thread root past cutoff with
    // any direct reply newer than the cutoff is preserved. The whole
    // unit (root + replies) survives. Strict semantic would delete the
    // root regardless and cascade-nuke the recent reply; the eventual
    // switch flips the NOT EXISTS clause in candidate_predicate.
    let pool = fresh_pool().await;
    let room = make_room(&pool, "thread-active", Some(30)).await;
    let root = make_message(&pool, room, "root", 60).await;
    let reply: i64 = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'recent reply', ?)",
    )
    .bind(room)
    .bind(root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    // Reply is 1 day old (well within cutoff).
    sqlx::query("UPDATE messages SET created_at = datetime('now', '-1 day') WHERE id = ?")
        .bind(reply)
        .execute(&pool)
        .await
        .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert!(message_exists(&pool, root).await);
    assert!(message_exists(&pool, reply).await);
}

#[tokio::test]
async fn thread_with_all_stale_replies_root_deletes_via_cascade() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "thread-stale", Some(30)).await;
    let root = make_message(&pool, room, "root", 60).await;
    let reply: i64 = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'stale reply', ?)",
    )
    .bind(room)
    .bind(root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("UPDATE messages SET created_at = datetime('now', '-45 days') WHERE id = ?")
        .bind(reply)
        .execute(&pool)
        .await
        .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    // Direct delete count is 1 (the root); the reply is removed via the
    // messages.parent_id ON DELETE CASCADE, which SQLite does not surface
    // in `rows_affected`. The integration assertion is "the table is now
    // empty", which is what compliance actually cares about.
    assert_eq!(stats.messages_deleted, 1);
    assert!(!message_exists(&pool, root).await);
    assert!(!message_exists(&pool, reply).await);
}

#[tokio::test]
async fn thread_reply_never_selected_directly_even_if_past_cutoff() {
    // If somehow a reply is past cutoff but its root is recent, the
    // predicate's `parent_id IS NULL` clause excludes the reply from the
    // candidate set so the reply survives. The thread's lifecycle is
    // tied to its root, not its individual replies.
    let pool = fresh_pool().await;
    let room = make_room(&pool, "thread-asym", Some(30)).await;
    let root = make_message(&pool, room, "recent root", 5).await;
    let old_reply: i64 = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'unusually old reply', ?)",
    )
    .bind(room)
    .bind(root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("UPDATE messages SET created_at = datetime('now', '-60 days') WHERE id = ?")
        .bind(old_reply)
        .execute(&pool)
        .await
        .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 0);
    assert!(message_exists(&pool, root).await);
    assert!(message_exists(&pool, old_reply).await);
}

// ----------------------------------------------------------------------
// Sweep behavior: settled-decision inclusions.
// soft-deleted, quarantined, system, and pinned messages all hard-delete
// past cutoff. None of these states is an escape hatch from retention.
// ----------------------------------------------------------------------

#[tokio::test]
async fn soft_deleted_message_past_cutoff_is_hard_deleted() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    let m = make_message(&pool, room, "moderation casualty", 60).await;
    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = 'mod' WHERE id = ?",
    )
    .bind(m)
    .execute(&pool)
    .await
    .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 1);
    assert!(!message_exists(&pool, m).await);
}

#[tokio::test]
async fn quarantined_message_past_cutoff_is_hard_deleted() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    let m = make_message(&pool, room, "held by link filter", 60).await;
    sqlx::query("UPDATE messages SET quarantined = 1 WHERE id = ?")
        .bind(m)
        .execute(&pool)
        .await
        .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 1);
    assert!(!message_exists(&pool, m).await);
}

#[tokio::test]
async fn system_message_past_cutoff_is_hard_deleted() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    let m = make_message(&pool, room, "alice joined", 60).await;
    sqlx::query("UPDATE messages SET is_system = 1 WHERE id = ?")
        .bind(m)
        .execute(&pool)
        .await
        .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 1);
    assert!(!message_exists(&pool, m).await);
}

#[tokio::test]
async fn pinned_message_past_cutoff_is_hard_deleted_with_no_exemption() {
    // No-pinned-exemption decision (compliance over UX). The pinned_messages
    // row CASCADES away with the message. The UI in a later commit will
    // warn admins that pins do not survive retention; this test enforces
    // the contract from the DB side.
    let pool = fresh_pool().await;
    let room = make_room(&pool, "r", Some(30)).await;
    let m = make_message(&pool, room, "important", 60).await;
    sqlx::query(
        "INSERT INTO pinned_messages (message_id, room_id, pinned_by) VALUES (?, ?, 'admin')",
    )
    .bind(m)
    .bind(room)
    .execute(&pool)
    .await
    .unwrap();

    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(stats.messages_deleted, 1);
    assert!(!message_exists(&pool, m).await);
    let pin_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE room_id = ?")
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pin_rows, 0);
}

// ----------------------------------------------------------------------
// Preview helper and its invariant: count_candidates_for_room must
// agree exactly with what sweep_once deletes for the same (room, days).
// This is the safety rail for the room-settings UI in a later commit.
// ----------------------------------------------------------------------

#[tokio::test]
async fn count_candidates_for_room_returns_expected_count() {
    let pool = fresh_pool().await;
    let room = make_room(&pool, "preview-r", Some(30)).await;
    for i in 0..3 {
        make_message(&pool, room, &format!("stale-{i}"), 60).await;
    }
    for i in 0..2 {
        make_message(&pool, room, &format!("recent-{i}"), 5).await;
    }

    let n = count_candidates_for_room(&pool, room, 30).await.unwrap();
    assert_eq!(n, 3, "only past-cutoff non-reply messages should count");
}

#[tokio::test]
async fn preview_count_equals_sweep_actual_delete() {
    // Load-bearing test: if these two ever diverge, the admin consents
    // to a blast radius that does not match what fires. Both call sites
    // share `candidate_predicate`; this test enforces the shared shape.
    let pool = fresh_pool().await;
    let room = make_room(&pool, "invariant", Some(30)).await;
    for i in 0..7 {
        make_message(&pool, room, &format!("stale-{i}"), 60).await;
    }
    // Plus a recent message that neither path should touch.
    make_message(&pool, room, "recent", 5).await;
    // Plus a thread with a recent reply (whose root is stale): neither
    // path should count or delete the root.
    let active_root = make_message(&pool, room, "thread root", 60).await;
    let recent_reply: i64 = sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, parent_id) \
         VALUES (?, 'u2', 'recent', ?)",
    )
    .bind(room)
    .bind(active_root)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("UPDATE messages SET created_at = datetime('now', '-2 days') WHERE id = ?")
        .bind(recent_reply)
        .execute(&pool)
        .await
        .unwrap();

    let preview = count_candidates_for_room(&pool, room, 30).await.unwrap();
    let stats = sweep_once(&pool).await.unwrap();

    assert_eq!(
        preview as u64, stats.messages_deleted,
        "preview count must equal what the sweep actually deletes (the shared-predicate invariant)",
    );
    assert_eq!(preview, 7, "only the 7 plain stale messages should count");
    assert!(
        message_exists(&pool, active_root).await,
        "active-thread root preserved"
    );
    assert!(message_exists(&pool, recent_reply).await);
}

// ----------------------------------------------------------------------
// API surface check on SweepStats for the spawn function in commit 4.
// ----------------------------------------------------------------------

#[tokio::test]
async fn sweep_once_default_stats_distinguishable_from_flag_disabled() {
    // The spawn function in commit 4 will key log output off these
    // fields; this test pins the shape so refactoring sweep_once cannot
    // accidentally invert the flag-disabled vs nothing-to-do signals.
    let pool = fresh_pool().await;
    let stats: SweepStats = sweep_once(&pool).await.unwrap();
    assert!(!stats.flag_disabled, "sweep_once never sets flag_disabled");
    assert_eq!(stats.messages_deleted, 0);
    assert_eq!(stats.rooms_touched, 0);
}
