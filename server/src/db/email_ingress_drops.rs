//! LC-207-OBSERVABILITY (#278): rolling log of email-ingress message drops.
//!
//! The IMAP poll loop's only diagnostic for "why didn't my email post?" was a
//! `WARN target=email_ingress::drop` log line. This table records each drop so
//! the admin settings page can show recent drops + a by-reason summary without
//! container-log access.
//!
//! Privacy: stores ONLY the structured `DropReason::as_str()`, the IMAP UID,
//! and a bounded non-body diagnostic `detail` (an error string or rate-limit
//! note). It never stores the message body, subject, or any correspondent
//! address. Rows are swept at 30 days by the hourly orphan sweeper, mirroring
//! the dedup table.

use sqlx::SqlitePool;

/// Cap on the stored diagnostic detail; the detail is a short error/rate-limit
/// note, never message content, but bounding it keeps a pathological error
/// string from bloating the row.
const MAX_DETAIL_LEN: usize = 200;

#[derive(Debug, Clone)]
pub struct DropRecord {
    pub dropped_at: String,
    pub reason: String,
    pub uid: Option<i64>,
    pub detail: Option<String>,
}

/// Record one dropped inbound message. `detail` is truncated to
/// `MAX_DETAIL_LEN` chars; an empty detail is stored as NULL.
pub async fn record(
    pool: &SqlitePool,
    reason: &str,
    uid: Option<i64>,
    detail: &str,
) -> sqlx::Result<()> {
    let detail: Option<String> = if detail.is_empty() {
        None
    } else {
        Some(detail.chars().take(MAX_DETAIL_LEN).collect())
    };
    sqlx::query("INSERT INTO email_ingress_drops (reason, uid, detail) VALUES (?, ?, ?)")
        .bind(reason)
        .bind(uid)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

/// Most recent `limit` drops, newest first.
pub async fn recent(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<DropRecord>> {
    let rows = sqlx::query_as::<_, (String, String, Option<i64>, Option<String>)>(
        "SELECT dropped_at, reason, uid, detail FROM email_ingress_drops \
         ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(dropped_at, reason, uid, detail)| DropRecord {
            dropped_at,
            reason,
            uid,
            detail,
        })
        .collect())
}

/// Count drops grouped by reason within the last `hours` hours, highest first.
/// Drives the at-a-glance summary line on the admin page.
pub async fn counts_by_reason(pool: &SqlitePool, hours: i64) -> sqlx::Result<Vec<(String, i64)>> {
    let cutoff = format!("-{hours} hours");
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT reason, COUNT(*) AS n FROM email_ingress_drops \
         WHERE dropped_at >= datetime('now', ?) GROUP BY reason ORDER BY n DESC, reason ASC",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Drop rows older than `days` days. Returns the deleted count for the orphan
/// sweeper's log line.
pub async fn sweep_old(pool: &SqlitePool, days: i64) -> sqlx::Result<u64> {
    let cutoff = format!("-{days} days");
    let res = sqlx::query("DELETE FROM email_ingress_drops WHERE dropped_at < datetime('now', ?)")
        .bind(&cutoff)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
