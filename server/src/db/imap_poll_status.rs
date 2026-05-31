//! LC-207-OBSERVABILITY (#278): runtime health of the IMAP poll loop.
//!
//! `spawn_email_poll` writes a singleton row (`id = 1`) on every tick so the
//! admin settings page can show "last poll at T, last success at T, N
//! consecutive failures, last error" without the operator needing
//! container-log access. This table holds NO credentials and NO message
//! content - operator config lives in `imap_inbox_config`.

use sqlx::SqlitePool;

#[derive(Debug, Clone, Default)]
pub struct ImapPollStatus {
    pub last_poll_at: Option<String>,
    pub last_ok_at: Option<String>,
    pub last_error: Option<String>,
    pub consecutive_failures: i64,
    pub last_fetched: i64,
    pub last_posted: i64,
    pub last_dropped: i64,
}

/// Record a successful tick: stamp `last_poll_at` + `last_ok_at`, clear the
/// error, reset the consecutive-failure counter, and store the tick counts.
pub async fn record_success(
    pool: &SqlitePool,
    fetched: i64,
    posted: i64,
    dropped: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO imap_poll_status \
           (id, last_poll_at, last_ok_at, last_error, consecutive_failures, \
            last_fetched, last_posted, last_dropped, updated_at) \
         VALUES (1, datetime('now'), datetime('now'), NULL, 0, ?, ?, ?, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
            last_poll_at = datetime('now'), \
            last_ok_at = datetime('now'), \
            last_error = NULL, \
            consecutive_failures = 0, \
            last_fetched = excluded.last_fetched, \
            last_posted = excluded.last_posted, \
            last_dropped = excluded.last_dropped, \
            updated_at = datetime('now')",
    )
    .bind(fetched)
    .bind(posted)
    .bind(dropped)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed tick: stamp `last_poll_at`, store the error, increment the
/// consecutive-failure counter. Leaves `last_ok_at` and the counts untouched
/// so the page still shows when the loop last actually succeeded.
pub async fn record_failure(pool: &SqlitePool, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO imap_poll_status \
           (id, last_poll_at, last_error, consecutive_failures, updated_at) \
         VALUES (1, datetime('now'), ?, 1, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
            last_poll_at = datetime('now'), \
            last_error = excluded.last_error, \
            consecutive_failures = imap_poll_status.consecutive_failures + 1, \
            updated_at = datetime('now')",
    )
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn read(pool: &SqlitePool) -> sqlx::Result<Option<ImapPollStatus>> {
    let row = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        "SELECT last_poll_at, last_ok_at, last_error, consecutive_failures, \
                last_fetched, last_posted, last_dropped \
         FROM imap_poll_status WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(
            last_poll_at,
            last_ok_at,
            last_error,
            consecutive_failures,
            last_fetched,
            last_posted,
            last_dropped,
        )| {
            ImapPollStatus {
                last_poll_at,
                last_ok_at,
                last_error,
                consecutive_failures,
                last_fetched,
                last_posted,
                last_dropped,
            }
        },
    ))
}
