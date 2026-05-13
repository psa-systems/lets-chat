//! Orphan-upload garbage collection. Hourly sweep removes `file_uploads` rows
//! where `message_id IS NULL` and `created_at` is older than the threshold.
//! Dedup-aware: the on-disk file is removed only when no other row shares
//! the same `storage_path`.
//!
//! Single-process invariant. The sibling count, the filesystem delete, and
//! the row delete all run inside one SQLite transaction opened with
//! `BEGIN IMMEDIATE`, which acquires the write lock immediately and blocks
//! concurrent uploads from inserting a sibling row between the COUNT and the
//! DELETE. The file delete happens INSIDE that critical section, BEFORE the
//! row delete, so a file-delete failure rolls back the row delete and the
//! next sweep retries cleanly. Today the scheduler is one tokio task in one
//! process; if this ever moves to a sidecar, the per-row transaction and
//! `BEGIN IMMEDIATE` are load-bearing under concurrency, not decorative.

use std::path::Path;

use sqlx::SqlitePool;

#[derive(Debug, Default)]
pub struct SweepStats {
    pub rows_deleted: u64,
    pub files_deleted: u64,
    pub errors: u64,
}

pub async fn run_orphan_sweep(
    pool: &SqlitePool,
    threshold_hours: i64,
) -> Result<SweepStats, sqlx::Error> {
    let candidates =
        crate::db::uploads::select_orphans_older_than(pool, threshold_hours).await?;
    let mut stats = SweepStats::default();
    for (id, storage_path) in candidates {
        match sweep_one(pool, id, &storage_path).await {
            Ok(file_removed) => {
                stats.rows_deleted += 1;
                if file_removed {
                    stats.files_deleted += 1;
                }
            }
            Err(e) => {
                stats.errors += 1;
                tracing::warn!(
                    error = %e,
                    row_id = id,
                    storage_path = %storage_path,
                    "orphan sweep row failed",
                );
            }
        }
    }
    Ok(stats)
}

async fn sweep_one(
    pool: &SqlitePool,
    id: i64,
    storage_path: &str,
) -> Result<bool, sqlx::Error> {
    let uploads_dir = crate::db::uploads_dir();
    let original = uploads_dir.join(storage_path);
    let preview_name = crate::uploads::preview_storage_name(storage_path);
    let preview = uploads_dir.join(&preview_name);

    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE TRANSACTION")
        .execute(&mut *conn)
        .await?;

    let result: Result<bool, sqlx::Error> = async {
        let siblings =
            crate::db::uploads::count_uploads_sharing_path(&mut *conn, storage_path, id).await?;
        let mut file_removed = false;
        if siblings == 0 {
            // File delete is inside the transaction, before the row delete,
            // so if it fails the rollback restores the row for retry. A file
            // that is already missing is treated as success: the goal is
            // consistency between filesystem and DB, not preserving files
            // that were never there.
            remove_if_exists(&original).await?;
            remove_if_exists(&preview).await?;
            file_removed = true;
        }
        crate::db::uploads::delete_upload_row(&mut *conn, id).await?;
        Ok(file_removed)
    }
    .await;

    match result {
        Ok(file_removed) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(file_removed)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

async fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
