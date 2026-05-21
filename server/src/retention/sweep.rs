//! Retention sweep entry points.
//!
//! `run_retention_sweep` is the production entry: it checks the env flag
//! and delegates to `sweep_once`. `sweep_once` is the unguarded body and
//! is what tests call. Both return [`SweepStats`].
//!
//! Transaction shape mirrors `crate::uploads::sweep::sweep_one`:
//! `BEGIN IMMEDIATE TRANSACTION` to take the writer lock up front, then
//! the candidate SELECT + DELETE inside the transaction, then COMMIT.
//! Rolls back on any error so a partial-cascade-then-fail never leaves
//! the chat DB in an inconsistent state.

use sqlx::{Row, SqlitePool};

use crate::retention::predicate::candidate_predicate;

/// Maximum number of messages a single sweep tick removes. Bounds the
/// first-run cliff: enabling retention on a room with months of history
/// could otherwise produce a thundering DELETE that holds the write lock
/// for seconds. With `LIMIT 500`, the backlog drains across multiple
/// ticks instead.
pub const SWEEP_LIMIT: i64 = 500;

const FLAG_ENV: &str = "LETS_CHAT_RETENTION_SWEEP_ENABLED";

/// Result of a single sweep tick.
#[derive(Debug, Default, Clone)]
pub struct SweepStats {
    /// Number of distinct rooms that lost at least one message.
    pub rooms_touched: usize,
    /// Total messages hard-deleted (excludes cascade-deleted descendants
    /// like thread replies; those vanish via the FK CASCADE in the same
    /// transaction but are not counted here).
    pub messages_deleted: u64,
    /// `(message_id, room_id)` pairs for every message the sweep
    /// directly deleted. The spawn function iterates this list AFTER
    /// the transaction commits to broadcast `MessagePurged` events,
    /// keeping channel writes out of the BEGIN IMMEDIATE critical
    /// section. Cascade-deleted descendants (thread replies, etc.)
    /// are not included; the client renders the parent's removal and
    /// the children's DOM nodes go with it if they were rendered nested.
    pub purged: Vec<(i64, i64)>,
    /// `true` iff the public `run_retention_sweep` early-returned because
    /// the env flag was not set. Tests call `sweep_once` and never see
    /// this set; production logs key off it to make the off-state visible.
    pub flag_disabled: bool,
}

/// True iff `LETS_CHAT_RETENTION_SWEEP_ENABLED` is set to `1` or `true`
/// (case-insensitive). Anything else, including unset, is `false`. The
/// destructive sweep stays dark until an operator sets the env var.
pub fn flag_enabled() -> bool {
    std::env::var(FLAG_ENV)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Production entry. Honors the env flag.
pub async fn run_retention_sweep(pool: &SqlitePool) -> Result<SweepStats, sqlx::Error> {
    if !flag_enabled() {
        return Ok(SweepStats {
            flag_disabled: true,
            ..Default::default()
        });
    }
    sweep_once(pool).await
}

/// Body of the sweep, without the flag check. Public so tests can
/// exercise the destructive path without touching process-wide env
/// state. Production callers go through `run_retention_sweep`.
pub async fn sweep_once(pool: &SqlitePool) -> Result<SweepStats, sqlx::Error> {
    let predicate = candidate_predicate("datetime('now', '-' || r.retention_days || ' days')");
    let select_sql = format!(
        "SELECT m.id, m.room_id \
         FROM messages m \
         JOIN rooms r ON r.id = m.room_id \
         WHERE r.retention_days IS NOT NULL \
           AND r.room_type != 'dm' \
           AND {predicate} \
         ORDER BY m.created_at ASC \
         LIMIT ?"
    );

    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE TRANSACTION")
        .execute(&mut *conn)
        .await?;

    let result: Result<SweepStats, sqlx::Error> = async {
        let rows = sqlx::query(&select_sql)
            .bind(SWEEP_LIMIT)
            .fetch_all(&mut *conn)
            .await?;

        if rows.is_empty() {
            return Ok(SweepStats::default());
        }

        let purged: Vec<(i64, i64)> = rows
            .iter()
            .map(|r| (r.get::<i64, _>("id"), r.get::<i64, _>("room_id")))
            .collect();
        let ids: Vec<i64> = purged.iter().map(|(id, _)| *id).collect();
        let rooms_touched: std::collections::HashSet<i64> =
            purged.iter().map(|(_, room_id)| *room_id).collect();

        // Dry-run instrumentation. Kept as a distinct statement
        // immediately before the DELETE so removing it after operator
        // confirmation is a clean single-statement deletion that cannot
        // accidentally touch the DELETE itself. Logs only the count and
        // room cardinality (not ids), so noise stays bounded even at
        // SWEEP_LIMIT-sized ticks.
        //
        // Remove this `tracing::info!` block once the sweep has run in
        // production for a few weeks and the counts match expectations.
        tracing::info!(
            count = ids.len(),
            rooms = rooms_touched.len(),
            "retention sweep would delete (dry-run instrumentation; remove after rollout confirms)",
        );

        let deleted = hard_delete_messages(&mut *conn, &ids).await?;
        Ok(SweepStats {
            rooms_touched: rooms_touched.len(),
            messages_deleted: deleted,
            purged,
            flag_disabled: false,
        })
    }
    .await;

    match result {
        Ok(stats) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(stats)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

/// Hard-delete a pre-filtered set of message ids.
///
/// Returns the number of `messages` rows actually deleted. Rows removed
/// via FK CASCADE (reactions, mentions, thread descendants, etc.) are
/// not counted in the return value because SQLite's `rows_affected`
/// reports only the directly-affected table.
///
/// Carved out so future retention-adjacent code (LC-67 ephemeral
/// messages, admin "purge user data" flows) can reuse the same DELETE
/// path without re-deriving the predicate. The predicate that produced
/// `ids` is the caller's responsibility; this function does not
/// re-validate it.
pub async fn hard_delete_messages<'e, E>(executor: E, ids: &[i64]) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!("DELETE FROM messages WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.execute(executor).await?.rows_affected())
}

/// Count messages that *would* be deleted in `room_id` if `retention_days`
/// were set to `days` and the sweep ran immediately. Used by the room-
/// settings preview handler to surface "this will permanently delete
/// approximately X messages" before the admin commits the policy change.
///
/// The result must agree with what `sweep_once` actually deletes for the
/// same room and same days value; the shared [`candidate_predicate`]
/// guarantees this by construction.
pub async fn count_candidates_for_room(
    pool: &SqlitePool,
    room_id: i64,
    days: i64,
) -> Result<i64, sqlx::Error> {
    // Compile-time literal cutoff expression; `days` flows through `?`.
    // See `candidate_predicate`'s safety invariant.
    let predicate = candidate_predicate("datetime('now', '-' || ? || ' days')");
    let sql = format!(
        "SELECT COUNT(*) \
         FROM messages m \
         WHERE m.room_id = ? \
           AND {predicate}"
    );
    // Bind order follows `?` appearance in the rendered SQL:
    // 1) room_id  2) days (for created_at < cutoff)  3) days (for NOT EXISTS)
    sqlx::query_scalar(&sql)
        .bind(room_id)
        .bind(days)
        .bind(days)
        .fetch_one(pool)
        .await
}
