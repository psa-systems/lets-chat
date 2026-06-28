//! LC-486: cached per-message LLM translations, keyed by (message_id, locale).

use sqlx::{Row, SqlitePool};

/// Return the cached translation of `message_id` into `locale`, if any.
pub async fn get_cached(
    pool: &SqlitePool,
    message_id: i64,
    locale: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT translated FROM message_translations WHERE message_id = ? AND locale = ?",
    )
    .bind(message_id)
    .bind(locale)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get("translated")))
}

/// Cache a translation (upsert; a re-translation overwrites).
pub async fn upsert(
    pool: &SqlitePool,
    message_id: i64,
    locale: &str,
    translated: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_translations (message_id, locale, translated) \
         VALUES (?, ?, ?) \
         ON CONFLICT(message_id, locale) DO UPDATE SET translated = excluded.translated, \
             created_at = datetime('now')",
    )
    .bind(message_id)
    .bind(locale)
    .bind(translated)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop all cached translations for a message. Called on edit so a stale
/// translation of the old body is never served.
pub async fn delete_for_message(pool: &SqlitePool, message_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM message_translations WHERE message_id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}
