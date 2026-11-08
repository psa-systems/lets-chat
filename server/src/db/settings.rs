use sqlx::{Row, SqlitePool};

/// Default upload cap when `settings.max_upload_bytes` is missing/unparseable.
/// Mirrors `routes::uploads::DEFAULT_MAX_UPLOAD_BYTES` (10 MiB).
pub const DEFAULT_MAX_UPLOAD_BYTES: i64 = 10 * 1024 * 1024;

/// Resolve the configured max upload size (bytes), falling back to the default.
/// LC-500: shared so the composer can surface the same cap to client-side
/// validation that the upload route enforces server-side.
pub async fn max_upload_bytes(pool: &SqlitePool) -> i64 {
    narrow_max_upload_bytes(get_setting(pool, "max_upload_bytes").await)
}

/// LC-1026: the pure narrowing of a `max_upload_bytes` read to a byte cap, split
/// out so a test can force the `Err` branch. A read error logs a WARN and falls
/// back to the default (the same silent-misread class LC-919/LC-1018 removed
/// elsewhere); a missing / unparseable / non-positive value falls back silently.
fn narrow_max_upload_bytes(result: Result<Option<String>, sqlx::Error>) -> i64 {
    let value = match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, key = "max_upload_bytes", "setting read failed; using default");
            None
        }
    };
    value
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.get("value")))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_all_settings(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.get("key"), r.get("value")))
        .collect())
}

/// LC-927: settings key used as a single-flight lock so the admin-forced
/// reindex and the scheduled tick can never run `help_docs::reindex_all_inner`
/// at the same time (they would otherwise interleave `delete_by_source` /
/// `upsert` writes and race the source-prune "not visited this run" set).
const REINDEX_LOCK_KEY: &str = "help_docs_reindex_lock";

/// Try to acquire the reindex single-flight lock. Returns `true` if this
/// caller now holds it (the caller must call [`release_reindex_lock`] on
/// every exit path once it is done), `false` if another run already holds it.
/// The `INSERT ... ON CONFLICT DO NOTHING` is atomic under SQLite's
/// single-writer semantics, so two overlapping callers can never both win.
pub async fn try_acquire_reindex_lock(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, '1') ON CONFLICT(key) DO NOTHING",
    )
    .bind(REINDEX_LOCK_KEY)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Release the reindex single-flight lock. Idempotent: safe to call even when
/// the lock is not currently held.
pub async fn release_reindex_lock(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(REINDEX_LOCK_KEY)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_max_upload_bytes_covers_value_cases_and_falls_back_on_err() {
        // LC-1026: value cases unchanged; a read Err falls back to the default
        // (with a WARN - a code guarantee, like the LC-1018 readers; asserting log
        // output needs a global tracing subscriber, the LC-782 cross-test flake).
        assert_eq!(narrow_max_upload_bytes(Ok(Some("1048576".into()))), 1_048_576);
        // Missing / unparseable / non-positive -> the default, silently.
        assert_eq!(narrow_max_upload_bytes(Ok(None)), DEFAULT_MAX_UPLOAD_BYTES);
        assert_eq!(
            narrow_max_upload_bytes(Ok(Some("nope".into()))),
            DEFAULT_MAX_UPLOAD_BYTES
        );
        assert_eq!(
            narrow_max_upload_bytes(Ok(Some("0".into()))),
            DEFAULT_MAX_UPLOAD_BYTES
        );
        assert_eq!(
            narrow_max_upload_bytes(Ok(Some("-5".into()))),
            DEFAULT_MAX_UPLOAD_BYTES
        );
        // The new branch: a read error falls back to the default.
        assert_eq!(
            narrow_max_upload_bytes(Err(sqlx::Error::RowNotFound)),
            DEFAULT_MAX_UPLOAD_BYTES
        );
    }
}
