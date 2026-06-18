//! LC-22: scratch table for in-flight bunyip OIDC dances.
//!
//! Created at `GET /auth/bunyip/start` and consumed (deleted) at
//! `GET /auth/bunyip/callback`. Rows older than 5 minutes are rejected on
//! consume; a background broom sweeps the table on a longer cadence.
//!
//! Cutover note (pure-RP): the dance has only one shape now (the login
//! flow), so the additive design's `redirect_after` discriminator is gone.

use sqlx::SqlitePool;

const TTL_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct PendingDance {
    pub state: String,
    pub code_verifier: String,
    pub nonce: String,
    pub created_at: i64,
}

pub async fn insert(
    pool: &SqlitePool,
    state: &str,
    code_verifier: &str,
    nonce: &str,
) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO oidc_pending (state, code_verifier, nonce, created_at) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(state)
    .bind(code_verifier)
    .bind(nonce)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically consume the row. Returns `Ok(Some(_))` only when the row was
/// present AND the TTL has not expired. Returns `Ok(None)` for missing /
/// expired / already-consumed rows, which the callback maps to a generic
/// rejection (we don't leak which case it was - the symptoms are the same
/// from a CSRF standpoint).
pub async fn take(pool: &SqlitePool, state: &str) -> Result<Option<PendingDance>, sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    let cutoff = now - TTL_SECS;
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT state, code_verifier, nonce, created_at \
         FROM oidc_pending WHERE state = ?",
    )
    .bind(state)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("DELETE FROM oidc_pending WHERE state = ?")
        .bind(state)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if row.3 < cutoff {
        return Ok(None);
    }
    Ok(Some(PendingDance {
        state: row.0,
        code_verifier: row.1,
        nonce: row.2,
        created_at: row.3,
    }))
}

/// Best-effort sweep of rows older than 1 hour. Called from the background
/// broom; failures are logged and otherwise ignored.
pub async fn sweep_stale(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let cutoff = chrono::Utc::now().timestamp() - 3600;
    let r = sqlx::query("DELETE FROM oidc_pending WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}
