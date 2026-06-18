//! LC-334: message reports. A member flags a message with a preset category and
//! an optional note; site admins triage the open queue at `/admin/reports`.

use sqlx::{Row, SqlitePool};

use crate::models::report::MessageReport;

/// Allowed report categories. Validated server-side; an unknown value is
/// rejected before insert. Kept in sync with the modal radio options and the
/// `report-category-*` i18n keys.
pub const REPORT_CATEGORIES: [&str; 4] = ["spam", "harassment", "inappropriate", "other"];

/// Maximum length of the optional free-text note (Unicode scalar count). The
/// note is stored as plain text and rendered escaped (never through the
/// markdown pipeline), so this is a row-growth bound, not an LC-153 render cap.
pub const MAX_REPORT_NOTE_CHARS: usize = 500;

/// True when `category` is one of the allowlisted values.
pub fn is_valid_category(category: &str) -> bool {
    REPORT_CATEGORIES.contains(&category)
}

/// File a report. Idempotent per `UNIQUE(message_id, reporter_id)`: a second
/// report by the same user for the same message is a no-op. Returns `true` when
/// a new row was inserted, `false` when an existing report suppressed it.
pub async fn create(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    reporter_id: &str,
    category: &str,
    note: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO message_reports (message_id, room_id, reporter_id, category, note) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (message_id, reporter_id) DO NOTHING",
    )
    .bind(message_id)
    .bind(room_id)
    .bind(reporter_id)
    .bind(category)
    .bind(note)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Open reports, newest first.
pub async fn list_open(pool: &SqlitePool) -> Result<Vec<MessageReport>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, message_id, room_id, reporter_id, category, note, status, handled_by, \
         created_at, handled_at \
         FROM message_reports WHERE status = 'open' ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_report).collect())
}

/// Count of open reports (drives the nav badge).
pub async fn count_open(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM message_reports WHERE status = 'open'")
        .fetch_one(pool)
        .await?;
    Ok(row.get("n"))
}

/// Resolve or dismiss an open report. Only transitions a report still in the
/// `open` state, so two admins acting on the same row do not double-handle it.
/// Returns `true` when a row was updated.
pub async fn set_status(
    pool: &SqlitePool,
    id: i64,
    status: &str,
    handled_by: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE message_reports \
         SET status = ?, handled_by = ?, handled_at = datetime('now') \
         WHERE id = ? AND status = 'open'",
    )
    .bind(status)
    .bind(handled_by)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

fn row_to_report(r: sqlx::sqlite::SqliteRow) -> MessageReport {
    MessageReport {
        id: r.get("id"),
        message_id: r.get("message_id"),
        room_id: r.get("room_id"),
        reporter_id: r.get("reporter_id"),
        category: r.get("category"),
        note: r.get("note"),
        status: r.get("status"),
        handled_by: r.get("handled_by"),
        created_at: r.get("created_at"),
        handled_at: r.get("handled_at"),
    }
}
