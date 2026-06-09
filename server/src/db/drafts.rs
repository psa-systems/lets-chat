//! Per-room message drafts: in-progress textarea contents persisted
//! per (user, room) so opening the room on another device restores the
//! half-typed message.
//!
//! See `migrations/chat/0047_message_drafts.sql` for the schema and the
//! rationale behind the LWW semantics and the cascade rules. The
//! handler that issues the upserts is `routes/drafts.rs`; the render
//! site that reads the body is the room/DM page handlers; the cleanup
//! sites are `routes/account.rs::purge_user_chat` (user delete),
//! `routes/room.rs::finalize_message_send` (clear-on-send), and
//! `routes/scheduled.rs::post_create` (clear-on-schedule).

use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

/// A draft row as read from the DB. `updated_at` is the SQLite
/// `"YYYY-MM-DD HH:MM:SS"` UTC string; the render site compares it
/// against `datetime('now', '-60 days')` to decide whether to render
/// the body or treat the draft as stale and lazily delete it.
#[derive(Debug, Clone)]
pub struct DraftRow {
    pub body: String,
    pub updated_at: String,
}

/// Read this user's draft for `room_id`. Returns `None` when no row
/// exists. The caller decides whether the row is stale (60-day rule
/// lives at the render site, not here, because "render-empty" and
/// "delete-the-row" are paired actions in the same code path).
pub async fn get(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<Option<DraftRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT body, updated_at FROM message_drafts WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| DraftRow {
        body: r.get("body"),
        updated_at: r.get("updated_at"),
    }))
}

/// Upsert this user's draft for `room_id`. Last-write-wins on
/// `updated_at`. The caller is responsible for the empty-body case
/// (PUT handler deletes instead of upserting an empty string) and for
/// the race-guard against send-then-resurrect (PUT handler refuses to
/// upsert when the body exactly matches a `messages` row from this
/// user in this room within the last 5 seconds). This function does
/// not re-validate either; it just writes.
pub async fn upsert(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    body: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_drafts (user_id, room_id, body, updated_at) \
         VALUES (?, ?, ?, datetime('now')) \
         ON CONFLICT (user_id, room_id) DO UPDATE SET \
             body = excluded.body, \
             updated_at = datetime('now')",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete this user's draft for `room_id`. Returns the number of rows
/// removed (0 if no draft existed). Best-effort callers (clear-on-send,
/// clear-on-schedule) discard the result; the PUT handler's empty-body
/// branch and the lazy-cleanup-on-load branch also discard it.
pub async fn delete(pool: &SqlitePool, user_id: &str, room_id: i64) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM message_drafts WHERE user_id = ? AND room_id = ?")
        .bind(user_id)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Delete every draft this user has, across every room. Takes an
/// `Executor` (not `&SqlitePool`) so it composes inside
/// `routes/account.rs::purge_user_chat`'s transaction next to the
/// existing scheduled / reminders / mentions cleanup queries.
pub async fn delete_for_user<'e, E>(executor: E, user_id: &str) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let res = sqlx::query("DELETE FROM message_drafts WHERE user_id = ?")
        .bind(user_id)
        .execute(executor)
        .await?;
    Ok(res.rows_affected())
}

/// Delete this user's draft from one specific room. Used by the
/// kick / remove-from-room paths in `routes/enclave.rs` to satisfy the
/// acceptance criterion that kicking removes the kicked user's drafts.
/// Gated on Nate's sign-off (the lingering-row reading is also
/// defensible); the function exists either way so commit 4 can wire it
/// in without touching this module.
pub async fn delete_for_user_in_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<u64, sqlx::Error> {
    delete(pool, user_id, room_id).await
}

/// LC-239: the set of `room_id`s in which this user has a fresh
/// (non-stale) draft. Drives the per-conversation sidebar draft
/// indicator built in `routes/mod.rs::load_sidebar`. Mirrors the
/// `max_age_days` freshness rule the render sites apply in
/// `get_fresh_or_purge`, so a stale draft never shows a badge; stale
/// rows are not deleted here (the lazy purge stays on the per-room
/// render path). `body != ''` is belt-and-suspenders: the PUT handler
/// already deletes on empty, so no empty-body row should exist.
pub async fn room_ids_with_drafts(
    pool: &SqlitePool,
    user_id: &str,
    max_age_days: i64,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query_scalar::<_, i64>(
        "SELECT room_id FROM message_drafts \
         WHERE user_id = ? AND body != '' \
           AND updated_at >= datetime('now', ?)",
    )
    .bind(user_id)
    .bind(format!("-{max_age_days} days"))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Read this user's draft for `room_id` and apply the lazy-cleanup
/// rule: if the row exists but is older than `max_age_days`, silently
/// delete it and return `None` (the caller renders an empty composer
/// as if no draft existed). Returns `Some(body)` for fresh drafts,
/// `None` for stale-or-absent.
///
/// The 60-day threshold lives at the call sites (`get_room`, `get_dm`)
/// so the value is visible at the policy boundary, not buried in the
/// db module.
///
/// **Intentional write-in-read-path.** A GET-render that finds a
/// stale draft also issues the DELETE in the same call. Cheaper than a
/// background sweep task for tiny rows (the LC-XXX retention-style
/// `spawn_*` infrastructure is overkill for kilobyte-scale data); the
/// cleanup is paid only by the user whose render would have shown the
/// stale draft. Idempotent: deleting the user's own row from their own
/// render. Do NOT refactor this into a separate cleanup-sweep task
/// unless the table ever grows large enough to justify the
/// infrastructure (it will not for any realistic chat-server scale).
pub async fn get_fresh_or_purge(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    max_age_days: i64,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT body, (updated_at < datetime('now', ?)) AS is_stale \
         FROM message_drafts WHERE user_id = ? AND room_id = ?",
    )
    .bind(format!("-{max_age_days} days"))
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    let is_stale: i64 = row.get("is_stale");
    if is_stale != 0 {
        let _ = delete(pool, user_id, room_id).await;
        return Ok(None);
    }
    Ok(Some(row.get("body")))
}
