//! Persistence for `scheduled_messages`. The dispatcher in
//! `crate::scheduled::dispatcher` (Task 3) calls `select_due`,
//! `try_claim`, `mark_dropped`, and `bump_attempt`; the routes in
//! `crate::routes::scheduled` (Task 4) call everything else.
//!
//! State derives from `delivered_at` + `failed_reason` at every query
//! site, no enum column. Keeping the predicates inline (rather than
//! behind a `ScheduledRow::state()` helper) avoids drift between the
//! Rust-side state check and the SQL.

use sqlx::{Row, SqliteConnection, SqlitePool};

pub struct ScheduledRow {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub body: String,
    pub scheduled_for: String,
    pub created_at: String,
    pub parent_id: Option<i64>,
    pub quote_id: Option<i64>,
    pub file_id: Option<i64>,
    pub delivered_at: Option<String>,
    pub failed_reason: Option<String>,
    pub attempt_count: i64,
    pub next_attempt_at: Option<String>,
}

fn row_to_scheduled(r: sqlx::sqlite::SqliteRow) -> ScheduledRow {
    ScheduledRow {
        id: r.get("id"),
        room_id: r.get("room_id"),
        user_id: r.get("user_id"),
        body: r.get("body"),
        scheduled_for: r.get("scheduled_for"),
        created_at: r.get("created_at"),
        parent_id: r.get("parent_id"),
        quote_id: r.get("quote_id"),
        file_id: r.get("file_id"),
        delivered_at: r.get("delivered_at"),
        failed_reason: r.get("failed_reason"),
        attempt_count: r.get("attempt_count"),
        next_attempt_at: r.get("next_attempt_at"),
    }
}

/// Insert a new pending scheduled message. Caller is responsible for the
/// pre-insert validation (lead-time bounds, room access, attachment
/// ownership, link filter, quote/parent existence); this function does
/// no checks beyond what the schema enforces. Returns the row id.
#[allow(clippy::too_many_arguments)]
pub async fn insert_scheduled(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
    scheduled_for: &str,
    parent_id: Option<i64>,
    quote_id: Option<i64>,
    file_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO scheduled_messages \
             (room_id, user_id, body, scheduled_for, parent_id, quote_id, file_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(body)
    .bind(scheduled_for)
    .bind(parent_id)
    .bind(quote_id)
    .bind(file_id)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Fetch one row by id regardless of state. Callers that want only
/// pending rows must check `delivered_at.is_none()` themselves.
pub async fn get_scheduled(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<ScheduledRow>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM scheduled_messages WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(row_to_scheduled))
}

/// All pending rows for a user, soonest first. Drives the main list on
/// the `/scheduled` management page.
pub async fn list_pending_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<ScheduledRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT * FROM scheduled_messages \
          WHERE user_id = ? AND delivered_at IS NULL \
          ORDER BY scheduled_for ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_scheduled).collect())
}

/// Most recent N dropped rows for a user, newest-first. Surfaces the
/// "Recently dropped" section of `/scheduled` so users see when (and
/// why) a scheduled message did not deliver.
pub async fn list_recent_dropped_for_user(
    pool: &SqlitePool,
    user_id: &str,
    limit: i64,
) -> Result<Vec<ScheduledRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT * FROM scheduled_messages \
          WHERE user_id = ? AND delivered_at IS NOT NULL AND failed_reason IS NOT NULL \
          ORDER BY delivered_at DESC \
          LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_scheduled).collect())
}

/// Update a pending row's editable fields (body, scheduled_for, file_id).
/// User scoping is enforced in the WHERE so a forged id from another
/// caller cannot edit someone else's row. Resets `attempt_count` and
/// `next_attempt_at` so an edit gives the dispatcher a clean retry.
/// Returns `false` when the row is missing, not owned by `user_id`, or
/// already delivered/dropped; the route layer turns that into 404/410.
pub async fn update_scheduled(
    pool: &SqlitePool,
    id: i64,
    user_id: &str,
    body: &str,
    scheduled_for: &str,
    file_id: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE scheduled_messages \
            SET body = ?, scheduled_for = ?, file_id = ?, \
                attempt_count = 0, next_attempt_at = NULL \
          WHERE id = ? AND user_id = ? AND delivered_at IS NULL",
    )
    .bind(body)
    .bind(scheduled_for)
    .bind(file_id)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Cancel a pending row. Hard delete, no audit trail (matches the ticket
/// decision). Same ownership scoping and `false`-on-miss semantics as
/// `update_scheduled`.
pub async fn delete_scheduled(
    pool: &SqlitePool,
    id: i64,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM scheduled_messages \
          WHERE id = ? AND user_id = ? AND delivered_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Drop every scheduled row authored by `user_id`, regardless of state.
/// Called from `routes/account.rs::purge_user_chat` inside the user-purge
/// transaction; takes `&mut SqliteConnection` so it shares the
/// transaction handle with the surrounding deletes. Returns the row
/// count for logging.
pub async fn delete_for_user(
    conn: &mut SqliteConnection,
    user_id: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM scheduled_messages WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    Ok(result.rows_affected())
}

/// Rows that are due and not currently in backoff cooldown. `LIMIT` caps
/// the per-tick batch so a long server outage (which leaves a large
/// backlog) cannot stampede push fan-out at startup; later ticks drain
/// what is left. Uses `idx_sched_due` for the scan + sort.
pub async fn select_due(pool: &SqlitePool, limit: i64) -> Result<Vec<ScheduledRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT * FROM scheduled_messages \
          WHERE delivered_at IS NULL \
            AND scheduled_for <= datetime('now') \
            AND (next_attempt_at IS NULL OR next_attempt_at <= datetime('now')) \
          ORDER BY scheduled_for ASC \
          LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_scheduled).collect())
}

/// Atomically claim a row for delivery by flipping `delivered_at` from
/// NULL to `now`. Returns `true` when exactly one row updated (claim
/// won), `false` when zero rows updated (someone else got there first
/// or the row no longer exists in pending state). Caller runs this
/// inside a `BEGIN IMMEDIATE` transaction shared with the messages
/// INSERT; on `false`, ROLLBACK and move on to the next row.
pub async fn try_claim(conn: &mut SqliteConnection, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE scheduled_messages SET delivered_at = datetime('now') \
          WHERE id = ? AND delivered_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Inside the same `BEGIN IMMEDIATE` transaction where `try_claim`
/// already set `delivered_at`, additionally set `failed_reason` so the
/// row reads as dropped (not delivered). The dispatcher calls this when
/// a re-validation check (room access, link filter, DM block) denies
/// delivery; the transaction then COMMITs and the management UI surfaces
/// the reason in the "Recently dropped" section.
pub async fn mark_dropped(
    conn: &mut SqliteConnection,
    id: i64,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE scheduled_messages SET failed_reason = ? WHERE id = ?")
        .bind(reason)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Record a dispatch failure (typically: the INSERT inside the claim
/// transaction returned an error, the transaction rolled back, the claim
/// was undone). Increments `attempt_count` and pushes `next_attempt_at`
/// forward by `backoff_secs` so `select_due` will skip the row until the
/// cooldown elapses. Restricted to non-delivered rows so a late call
/// after a successful delivery is a no-op.
pub async fn bump_attempt(
    pool: &SqlitePool,
    id: i64,
    backoff_secs: i64,
) -> Result<(), sqlx::Error> {
    let modifier = format!("+{backoff_secs} seconds");
    sqlx::query(
        "UPDATE scheduled_messages \
            SET attempt_count = attempt_count + 1, \
                next_attempt_at = datetime('now', ?) \
          WHERE id = ? AND delivered_at IS NULL",
    )
    .bind(modifier)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
