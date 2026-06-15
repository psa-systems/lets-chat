//! LC-304: per-user highlight words. A message containing one of a user's
//! words is treated like an @mention (see `routes::room::append_keyword_targets`,
//! which loads `all()` and matches against the posted body). Words live in
//! `auth.db`; the match produces a row in `chat.db::mentions`.
use sqlx::{Row, SqlitePool};

/// Max words per user and max length per word, enforced on the write path
/// (the schema does not constrain them).
pub const MAX_KEYWORDS_PER_USER: usize = 25;
pub const MAX_KEYWORD_LEN: usize = 50;

/// A user's words, alphabetical, for the settings chip list.
pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT word FROM notification_keywords WHERE user_id = ? ORDER BY word COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("word"))
        .collect())
}

/// Every `(user_id, word)` pair across all users, for the post-time matcher.
/// v1 scale: a self-hosted instance's keyword set is small, and the caller
/// only resolves room members when a word actually matched the body, so this
/// stays cheap. If the table grows large this should be scoped by room
/// membership instead.
pub async fn all(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id, word FROM notification_keywords")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("user_id"), r.get::<String, _>("word")))
        .collect())
}

pub async fn count(pool: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM notification_keywords WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("n"))
}

/// Insert a word for a user. Idempotent (UNIQUE(user_id, word) + OR IGNORE),
/// so re-adding an existing word is a no-op.
pub async fn add(pool: &SqlitePool, user_id: &str, word: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO notification_keywords (user_id, word) VALUES (?, ?)")
        .bind(user_id)
        .bind(word)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, user_id: &str, word: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM notification_keywords WHERE user_id = ? AND word = ?")
        .bind(user_id)
        .bind(word)
        .execute(pool)
        .await?;
    Ok(())
}
