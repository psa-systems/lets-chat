//! LC-97: admin analytics. Daily metrics are pre-aggregated into
//! `analytics_daily` (chat.db) by the background aggregator so the
//! dashboard renders from a few indexed reads instead of scanning the
//! whole messages table on every page view.
//!
//! Privacy: every metric stored here is a count. We never persist or
//! render per-user activity, only rolled-up totals. DAU/MAU are defined
//! as "distinct users who sent a message", which is derivable from
//! historical data (so a fresh install backfills its entire history on
//! first run) and never exposes who was active.
//!
//! Cross-pool: message/room counts come from chat.db; signups come from
//! auth.db. The aggregator reads both and writes the rollup into
//! chat.db's `analytics_daily`.

use sqlx::{Row, SqlitePool};

/// Metric keys stored in `analytics_daily.metric`. Kept as `&str`
/// constants so the aggregator and the dashboard query agree on spelling.
pub const METRIC_MESSAGES: &str = "messages";
pub const METRIC_DAU: &str = "dau";
pub const METRIC_MAU: &str = "mau";
pub const METRIC_ACTIVE_ROOMS: &str = "active_rooms";
pub const METRIC_SIGNUPS: &str = "signups";

/// One point in a time series: a UTC date (`YYYY-MM-DD`) and its count.
#[derive(Debug, Clone)]
pub struct DayPoint {
    pub date: String,
    pub value: i64,
}

/// A signup-cohort row for the retention triangle. `retained[k]` is the
/// percentage (0-100) of the cohort that sent a message in the k-th week
/// after signup (`retained[0]` is the signup week itself, always 100 for
/// a non-empty cohort if anyone posted, but computed honestly). `None`
/// marks weeks that have not happened yet for this cohort.
#[derive(Debug, Clone)]
pub struct RetentionCohort {
    pub week_label: String,
    pub size: i64,
    pub retained: Vec<Option<i64>>,
}

/// Recompute and upsert every metric for a single UTC `date`
/// (`YYYY-MM-DD`). Idempotent: re-running overwrites the day's rows, so
/// the hourly tick can keep "today" fresh and an admin can force a
/// recompute without creating duplicates.
pub async fn recompute_day(auth: &SqlitePool, chat: &SqlitePool, date: &str) -> sqlx::Result<()> {
    // Messages sent that day (exclude soft-deleted and system messages
    // so the count reflects real human activity).
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages \
         WHERE date(created_at) = ?1 AND deleted_at IS NULL AND COALESCE(is_system, 0) = 0",
    )
    .bind(date)
    .fetch_one(chat)
    .await?;

    let dau: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM messages \
         WHERE date(created_at) = ?1 AND deleted_at IS NULL AND COALESCE(is_system, 0) = 0",
    )
    .bind(date)
    .fetch_one(chat)
    .await?;

    let active_rooms: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT room_id) FROM messages \
         WHERE date(created_at) = ?1 AND deleted_at IS NULL AND COALESCE(is_system, 0) = 0",
    )
    .bind(date)
    .fetch_one(chat)
    .await?;

    // MAU: distinct senders over the trailing 30 days ending on `date`
    // (inclusive). Rolling window, recomputed per day.
    let mau: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM messages \
         WHERE date(created_at) > date(?1, '-30 days') AND date(created_at) <= ?1 \
           AND deleted_at IS NULL AND COALESCE(is_system, 0) = 0",
    )
    .bind(date)
    .fetch_one(chat)
    .await?;

    let signups: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE date(created_at) = ?1")
        .bind(date)
        .fetch_one(auth)
        .await?;

    for (metric, value) in [
        (METRIC_MESSAGES, messages),
        (METRIC_DAU, dau),
        (METRIC_ACTIVE_ROOMS, active_rooms),
        (METRIC_MAU, mau),
        (METRIC_SIGNUPS, signups),
    ] {
        upsert(chat, date, metric, value).await?;
    }
    Ok(())
}

async fn upsert(chat: &SqlitePool, date: &str, metric: &str, value: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO analytics_daily (date, scope_kind, scope_id, metric, value) \
         VALUES (?1, 'global', 0, ?2, ?3) \
         ON CONFLICT(date, scope_kind, scope_id, metric) DO UPDATE SET value = excluded.value",
    )
    .bind(date)
    .bind(metric)
    .bind(value)
    .execute(chat)
    .await?;
    Ok(())
}

/// Earliest UTC date with any data (message or signup), or `None` on a
/// totally empty install. Used to bound the startup backfill.
pub async fn earliest_activity_date(
    auth: &SqlitePool,
    chat: &SqlitePool,
) -> sqlx::Result<Option<String>> {
    let msg: Option<String> =
        sqlx::query_scalar("SELECT MIN(date(created_at)) FROM messages WHERE deleted_at IS NULL")
            .fetch_one(chat)
            .await?;
    let usr: Option<String> = sqlx::query_scalar("SELECT MIN(date(created_at)) FROM users")
        .fetch_one(auth)
        .await?;
    Ok([msg, usr].into_iter().flatten().min())
}

/// Backfill every day from `earliest_activity_date` through `today`
/// (inclusive). Runs once at startup so a fresh deploy has its full
/// history immediately. Cheap: each day is five aggregate counts, and
/// installs have at most a few thousand days of history.
pub async fn backfill(auth: &SqlitePool, chat: &SqlitePool, today: &str) -> sqlx::Result<u32> {
    let Some(start) = earliest_activity_date(auth, chat).await? else {
        return Ok(0);
    };
    let mut day = start;
    let mut count = 0u32;
    while day.as_str() <= today {
        recompute_day(auth, chat, &day).await?;
        count += 1;
        // Advance one calendar day via SQLite's date math so we don't
        // re-implement leap-year/month-length logic in Rust.
        day = sqlx::query_scalar("SELECT date(?1, '+1 day')")
            .bind(&day)
            .fetch_one(chat)
            .await?;
    }
    Ok(count)
}

/// Time series for one metric over `[from, to]` (inclusive UTC dates).
/// Days with no stored row are omitted; the dashboard fills gaps when it
/// builds the chart axis so a quiet day reads as zero, not a break.
pub async fn series(
    chat: &SqlitePool,
    metric: &str,
    from: &str,
    to: &str,
) -> sqlx::Result<Vec<DayPoint>> {
    let rows = sqlx::query(
        "SELECT date, value FROM analytics_daily \
         WHERE metric = ?1 AND scope_kind = 'global' AND scope_id = 0 \
           AND date >= ?2 AND date <= ?3 \
         ORDER BY date",
    )
    .bind(metric)
    .bind(from)
    .bind(to)
    .fetch_all(chat)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DayPoint {
            date: r.get::<String, _>("date"),
            value: r.get::<i64, _>("value"),
        })
        .collect())
}

/// Retention triangle for the most recent `weeks` signup cohorts. A
/// cohort is everyone who registered in a given ISO-ish week
/// (`strftime('%Y-%W')`); `retained[k]` is the percent of that cohort who
/// sent at least one message in the k-th week after signup.
///
/// Cross-pool and computed on demand (not pre-aggregated): cohort
/// membership comes from auth.db, weekly message activity from chat.db.
/// Bounded to `weeks` cohorts so the work stays small for an admin page
/// load.
pub async fn retention(
    auth: &SqlitePool,
    chat: &SqlitePool,
    weeks: usize,
) -> sqlx::Result<Vec<RetentionCohort>> {
    // Cohort = signup week. Grab the most recent `weeks` cohorts with the
    // ids that belong to each, oldest-first for display.
    let cohort_rows = sqlx::query(
        "SELECT strftime('%Y-%W', created_at) AS wk, MIN(date(created_at)) AS wk_start, \
                GROUP_CONCAT(id) AS ids, COUNT(*) AS n \
         FROM users \
         GROUP BY wk \
         ORDER BY wk DESC \
         LIMIT ?1",
    )
    .bind(weeks as i64)
    .fetch_all(auth)
    .await?;

    let mut cohorts: Vec<RetentionCohort> = Vec::new();
    // Reverse so oldest cohort is first (top of the triangle).
    for row in cohort_rows.into_iter().rev() {
        let wk_start: String = row.get("wk_start");
        let size: i64 = row.get("n");
        let ids_csv: String = row.get("ids");
        let ids: Vec<&str> = ids_csv.split(',').collect();

        // How many whole weeks have elapsed since this cohort's week
        // start, capped at `weeks` columns so the triangle stays bounded.
        let weeks_elapsed: i64 = sqlx::query_scalar(
            "SELECT CAST((julianday(date('now')) - julianday(?1)) / 7 AS INTEGER)",
        )
        .bind(&wk_start)
        .fetch_one(auth)
        .await?;
        let cols = ((weeks_elapsed + 1).max(1) as usize).min(weeks);

        let mut retained: Vec<Option<i64>> = Vec::with_capacity(weeks);
        for k in 0..weeks {
            if k >= cols {
                retained.push(None);
                continue;
            }
            // Window for the k-th week after the cohort's week start.
            let lo = format!("+{} days", k * 7);
            let hi = format!("+{} days", (k + 1) * 7);
            // IN-list bound by cohort size; fine for an on-demand admin view.
            let placeholders = std::iter::repeat("?")
                .take(ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT COUNT(DISTINCT user_id) FROM messages \
                 WHERE date(created_at) >= date(?1, ?2) AND date(created_at) < date(?1, ?3) \
                   AND deleted_at IS NULL AND user_id IN ({placeholders})"
            );
            let mut q = sqlx::query_scalar::<_, i64>(&sql)
                .bind(&wk_start)
                .bind(&lo)
                .bind(&hi);
            for id in &ids {
                q = q.bind(*id);
            }
            let active: i64 = q.fetch_one(chat).await?;
            let pct = if size > 0 { (active * 100) / size } else { 0 };
            retained.push(Some(pct));
        }

        cohorts.push(RetentionCohort {
            week_label: wk_start,
            size,
            retained,
        });
    }
    Ok(cohorts)
}
