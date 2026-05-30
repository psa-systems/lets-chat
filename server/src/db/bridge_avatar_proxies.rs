//! LC-78-AVATAR-PROXY: storage for the bridge-avatar fetch cache.
//!
//! Each row records one foreign avatar URL we've been asked to proxy: the
//! cache key (`hash` = sha256 of the canonical URL), the URL itself, fetch
//! status, and metadata about the cached bytes if the fetch succeeded.
//!
//! The struct + helpers live inline here (no separate models/ entry) because
//! the row is consumed only by the proxy endpoint + the GC sweep, never by
//! a render-facing view. Same shape as `db::imap_config`.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct BridgeAvatarProxy {
    pub hash: String,
    pub foreign_url: String,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub fetched_at: Option<String>,
    pub last_seen_at: String,
    /// State machine: `pending` -> `ok` on successful fetch, `pending` ->
    /// `failed` on any rejection path (SSRF / HTTP error / size cap / sniff
    /// mismatch / decoder rejection / disk error). `failed` is terminal in
    /// v2; render falls back to initials forever.
    pub fetch_status: String,
    pub failure_reason: Option<String>,
}

fn map(row: &sqlx::sqlite::SqliteRow) -> BridgeAvatarProxy {
    BridgeAvatarProxy {
        hash: row.get("hash"),
        foreign_url: row.get("foreign_url"),
        content_type: row.get("content_type"),
        byte_size: row.get("byte_size"),
        fetched_at: row.get("fetched_at"),
        last_seen_at: row.get("last_seen_at"),
        fetch_status: row.get("fetch_status"),
        failure_reason: row.get("failure_reason"),
    }
}

/// Insert a `pending` row for a foreign URL, or bump `last_seen_at` on an
/// existing row (regardless of its `fetch_status`). Returns `true` if a new
/// row was inserted; `false` if an existing row's `last_seen_at` was
/// refreshed. The boolean lets the caller decide whether to spawn a fresh
/// fetch task (only on true) vs. let the existing row stand.
pub async fn upsert_pending(
    pool: &SqlitePool,
    hash: &str,
    foreign_url: &str,
) -> Result<bool, sqlx::Error> {
    // INSERT OR IGNORE distinguishes new-row (rows_affected = 1) from
    // existing-row (rows_affected = 0). SQLite's ON CONFLICT DO UPDATE
    // reports rows_affected = 1 in both cases, which is ambiguous.
    let result = sqlx::query(
        "INSERT OR IGNORE INTO bridge_avatar_proxies (hash, foreign_url) VALUES (?, ?)",
    )
    .bind(hash)
    .bind(foreign_url)
    .execute(pool)
    .await?;
    if result.rows_affected() == 1 {
        // Brand new row; last_seen_at defaults to datetime('now').
        return Ok(true);
    }
    // Existing row: bump last_seen_at so the GC sweep keeps it alive.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now') WHERE hash = ?")
        .bind(hash)
        .execute(pool)
        .await?;
    Ok(false)
}

pub async fn find_by_hash(
    pool: &SqlitePool,
    hash: &str,
) -> Result<Option<BridgeAvatarProxy>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT hash, foreign_url, content_type, byte_size, fetched_at, last_seen_at, fetch_status, failure_reason \
         FROM bridge_avatar_proxies WHERE hash = ?",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map))
}

/// Mark a previously-pending row as successfully fetched. Records the
/// sniffed content type + the on-disk byte size + sets fetched_at to now.
/// Returns `false` if no row existed (caller should treat as a race).
pub async fn mark_ok(
    pool: &SqlitePool,
    hash: &str,
    content_type: &str,
    byte_size: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE bridge_avatar_proxies \
         SET fetch_status = 'ok', content_type = ?, byte_size = ?, fetched_at = datetime('now'), failure_reason = NULL \
         WHERE hash = ?",
    )
    .bind(content_type)
    .bind(byte_size)
    .bind(hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark a previously-pending row as failed. Stores the short failure reason
/// (kept under 256 chars by the caller). `failed` is terminal in v2; render
/// falls back to initials via the `<img onerror>` path.
pub async fn mark_failed(pool: &SqlitePool, hash: &str, reason: &str) -> Result<bool, sqlx::Error> {
    let reason = if reason.len() > 256 {
        &reason[..256]
    } else {
        reason
    };
    let result = sqlx::query(
        "UPDATE bridge_avatar_proxies \
         SET fetch_status = 'failed', failure_reason = ? \
         WHERE hash = ?",
    )
    .bind(reason)
    .bind(hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// LC-207: aggregate cache stats for the admin diagnostic page. One query
/// with conditional SUMs (SQLite booleans are 1/0, so `SUM(cond)` counts).
/// `total_bytes` sums `byte_size`, which is non-NULL only on `ok` rows, so
/// it naturally reflects bytes-on-disk. `oldest_last_seen` is `MIN`
/// (`None` over an empty table). Covered by the
/// `(fetch_status, last_seen_at)` index.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub total: i64,
    pub ok: i64,
    pub pending: i64,
    pub failed: i64,
    /// Pending rows older than `PENDING_STALE_SECS` by `last_seen_at`: the
    /// orphan-pending anomaly counter.
    pub pending_stale: i64,
    pub total_bytes: i64,
    pub oldest_last_seen: Option<String>,
}

/// Stale-pending display threshold = the sweep's pending threshold (600s,
/// `bridge_avatar::sweep_tick` arg in `main.rs`) + one sweep interval (3600s,
/// `spawn_orphan_sweeper`'s `TICK_SECS`) = 4200s. A pending row older than
/// this has had a full sweep opportunity pass without being flipped to
/// `failed`, so it is genuinely anomalous (sweep broken / restart loop), not
/// normal sweep lag. Re-derive if either sweep constant changes. (Those
/// constants are a bare literal arg + a function-local const, so a parity
/// meta-test would require plumbing both out; the comment carries it.)
const PENDING_STALE_SECS: i64 = 4200;

pub async fn cache_stats(pool: &SqlitePool) -> Result<CacheStats, sqlx::Error> {
    let cutoff = format!("-{PENDING_STALE_SECS} seconds");
    let row = sqlx::query(
        "SELECT COUNT(*) AS total, \
         COALESCE(SUM(fetch_status = 'ok'), 0) AS ok, \
         COALESCE(SUM(fetch_status = 'pending'), 0) AS pending, \
         COALESCE(SUM(fetch_status = 'failed'), 0) AS failed, \
         COALESCE(SUM(fetch_status = 'pending' AND last_seen_at < datetime('now', ?)), 0) AS pending_stale, \
         COALESCE(SUM(byte_size), 0) AS total_bytes, \
         MIN(last_seen_at) AS oldest_last_seen \
         FROM bridge_avatar_proxies",
    )
    .bind(cutoff)
    .fetch_one(pool)
    .await?;
    Ok(CacheStats {
        total: row.get("total"),
        ok: row.get("ok"),
        pending: row.get("pending"),
        failed: row.get("failed"),
        pending_stale: row.get("pending_stale"),
        total_bytes: row.get("total_bytes"),
        oldest_last_seen: row.get("oldest_last_seen"),
    })
}

/// LC-207: the most-recently-referenced `failed` rows for the admin
/// diagnostic page. Ordered by `last_seen_at DESC` because that is the only
/// non-NULL timestamp a failed row carries (`fetched_at` is set on `mark_ok`
/// only) AND it surfaces the page's actual subjects first: a recently-seen
/// failure is an avatar users are actively hitting that renders as initials.
pub async fn recent_failures(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<BridgeAvatarProxy>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT hash, foreign_url, content_type, byte_size, fetched_at, last_seen_at, fetch_status, failure_reason \
         FROM bridge_avatar_proxies WHERE fetch_status = 'failed' ORDER BY last_seen_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map).collect())
}

/// GC: hashes of rows whose `last_seen_at` is older than the retention
/// threshold AND no live `messages.bridge_foreign_avatar` row references
/// them. Caller deletes the rows + the disk files in the same sweep tick.
/// `older_than_days` is the operator-set retention threshold; default 30.
pub async fn sweep_unreferenced(
    pool: &SqlitePool,
    older_than_days: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let cutoff = format!("-{older_than_days} days");
    let rows = sqlx::query(
        "SELECT p.hash FROM bridge_avatar_proxies p \
         WHERE p.last_seen_at < datetime('now', ?) \
           AND NOT EXISTS ( \
               SELECT 1 FROM messages m WHERE m.bridge_foreign_avatar = p.hash \
           )",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("hash")).collect())
}

/// Delete a hash's row. Caller should `unlink` the disk file in the same
/// step; this only touches the DB.
pub async fn delete_by_hash(pool: &SqlitePool, hash: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM bridge_avatar_proxies WHERE hash = ?")
        .bind(hash)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Crash-recovery sweep: mark rows stuck in `pending` for longer than the
/// fetch-timeout window as `failed`. A row sits in `pending` only between
/// the POST that inserted it and the `mark_ok` / `mark_failed` call at the
/// end of the fetch task; longer than that means the task crashed or the
/// process restarted before it completed.
pub async fn sweep_pending_orphans(
    pool: &SqlitePool,
    older_than_secs: i64,
) -> Result<u64, sqlx::Error> {
    let cutoff = format!("-{older_than_secs} seconds");
    let result = sqlx::query(
        "UPDATE bridge_avatar_proxies \
         SET fetch_status = 'failed', failure_reason = 'orphaned pending fetch (process restart?)' \
         WHERE fetch_status = 'pending' AND last_seen_at < datetime('now', ?)",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
