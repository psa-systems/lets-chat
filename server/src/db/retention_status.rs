//! LC-207-OBSERVABILITY (#278): runtime status of the message retention sweep.
//!
//! `spawn_message_retention_sweeper` writes a singleton row (`id = 1`) on every
//! tick so an operator who enabled the destructive
//! `LETS_CHAT_RETENTION_SWEEP_ENABLED` can confirm from the admin settings page
//! that the sweep ran and how much it deleted, without container-log access.
//! `last_run_at` is the last completed (Ok) tick (including a zero-delete one);
//! `last_error` is the most recent failure.

use sqlx::SqlitePool;

#[derive(Debug, Clone, Default)]
pub struct RetentionSweepStatus {
    pub last_run_at: Option<String>,
    pub last_rooms_touched: i64,
    pub last_messages_deleted: i64,
    pub total_messages_deleted: i64,
    pub runs: i64,
    pub last_error: Option<String>,
}

/// Record a completed sweep tick (success). Updates the last-run snapshot,
/// accumulates the lifetime delete total + run count, and clears `last_error`.
/// Called on every Ok tick, including zero-delete ones, so the page can show
/// "ran at T, deleted nothing" rather than looking stuck.
pub async fn record_run(
    pool: &SqlitePool,
    rooms_touched: i64,
    messages_deleted: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO retention_sweep_status \
           (id, last_run_at, last_rooms_touched, last_messages_deleted, \
            total_messages_deleted, runs, last_error, updated_at) \
         VALUES (1, datetime('now'), ?, ?, ?, 1, NULL, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
            last_run_at = datetime('now'), \
            last_rooms_touched = excluded.last_rooms_touched, \
            last_messages_deleted = excluded.last_messages_deleted, \
            total_messages_deleted = retention_sweep_status.total_messages_deleted + excluded.total_messages_deleted, \
            runs = retention_sweep_status.runs + 1, \
            last_error = NULL, \
            updated_at = datetime('now')",
    )
    .bind(rooms_touched)
    .bind(messages_deleted)
    .bind(messages_deleted)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed sweep tick: store the error without touching `last_run_at`,
/// the delete counters, or the run count (a failed tick is not a completed
/// run).
pub async fn record_error(pool: &SqlitePool, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO retention_sweep_status (id, last_error, updated_at) \
         VALUES (1, ?, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
            last_error = excluded.last_error, \
            updated_at = datetime('now')",
    )
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn read(pool: &SqlitePool) -> sqlx::Result<Option<RetentionSweepStatus>> {
    let row = sqlx::query_as::<_, (Option<String>, i64, i64, i64, i64, Option<String>)>(
        "SELECT last_run_at, last_rooms_touched, last_messages_deleted, \
                total_messages_deleted, runs, last_error \
         FROM retention_sweep_status WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(
            last_run_at,
            last_rooms_touched,
            last_messages_deleted,
            total_messages_deleted,
            runs,
            last_error,
        )| {
            RetentionSweepStatus {
                last_run_at,
                last_rooms_touched,
                last_messages_deleted,
                total_messages_deleted,
                runs,
                last_error,
            }
        },
    ))
}
