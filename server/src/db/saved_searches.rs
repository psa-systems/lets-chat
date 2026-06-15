//! LC-312: saved message-search queries (per user). The query string is also
//! its display label; re-running fills the search box client-side and lets the
//! existing `/search` FTS + operator + access-gating path run unchanged.
use sqlx::{Row, SqlitePool};

/// Max saved searches per user, enforced on the write path.
pub const MAX_SAVED_SEARCHES: usize = 50;

/// A user's saved queries, most recent first.
pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT query FROM saved_searches WHERE user_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("query"))
        .collect())
}

pub async fn count(pool: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM saved_searches WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("n"))
}

/// Save a query. Idempotent (UNIQUE(user_id, query) + OR IGNORE).
pub async fn add(pool: &SqlitePool, user_id: &str, query: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO saved_searches (user_id, query) VALUES (?, ?)")
        .bind(user_id)
        .bind(query)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, user_id: &str, query: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM saved_searches WHERE user_id = ? AND query = ?")
        .bind(user_id)
        .bind(query)
        .execute(pool)
        .await?;
    Ok(())
}
