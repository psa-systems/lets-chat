//! LC-529: reaction "highlights" recap. A non-LLM aggregate over
//! `message_reactions`: the most-reacted messages in a room over a recent
//! window, so a member who was heads-down can catch up on what popped. Ranked
//! by reactions RECEIVED IN THE WINDOW (not message age), so an older message
//! that got a fresh burst still surfaces.

use std::collections::HashMap;

use sqlx::{Row, SqlitePool};

/// One highlighted message: its author, body, and the reaction count within
/// the window. `emoji` breakdown is resolved separately in bulk.
#[derive(Debug, Clone)]
pub struct Highlight {
    pub message_id: i64,
    pub user_id: String,
    pub body: String,
    pub created_at: String,
    pub total: i64,
    /// `(emoji, count)` pairs, most-reacted first. Custom-emoji reactions
    /// appear as their raw `:shortcode:` text (recap is a lightweight view).
    pub emojis: Vec<(String, i64)>,
}

/// The top `limit` messages in `room_id` by reactions received since
/// `window` (a SQLite datetime modifier like `"-7 days"`). Messages with no
/// reactions in the window are excluded; soft-deleted messages are skipped.
pub async fn top_reacted(
    pool: &SqlitePool,
    room_id: i64,
    window: &str,
    limit: i64,
) -> sqlx::Result<Vec<Highlight>> {
    let rows = sqlx::query(
        "SELECT m.id AS id, m.user_id AS user_id, m.body AS body, m.created_at AS created_at, \
                COUNT(*) AS cnt \
         FROM messages m \
         JOIN message_reactions r ON r.message_id = m.id \
         WHERE m.room_id = ? AND m.deleted_at IS NULL \
           AND r.created_at >= datetime('now', ?) \
         GROUP BY m.id \
         ORDER BY cnt DESC, m.created_at DESC \
         LIMIT ?",
    )
    .bind(room_id)
    .bind(window)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut highlights: Vec<Highlight> = rows
        .into_iter()
        .map(|r| Highlight {
            message_id: r.get("id"),
            user_id: r.get("user_id"),
            body: r.get("body"),
            created_at: r.get("created_at"),
            total: r.get("cnt"),
            emojis: Vec::new(),
        })
        .collect();
    if highlights.is_empty() {
        return Ok(highlights);
    }

    // Bulk per-emoji breakdown for exactly the highlighted messages, same
    // window, so the chips match the ranking count.
    let ids: Vec<i64> = highlights.iter().map(|h| h.message_id).collect();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT message_id, emoji, COUNT(*) AS c FROM message_reactions \
         WHERE message_id IN ({placeholders}) AND created_at >= datetime('now', ?) \
         GROUP BY message_id, emoji ORDER BY c DESC, emoji",
    );
    let mut q = sqlx::query(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    q = q.bind(window);
    let brk = q.fetch_all(pool).await?;

    let mut by_msg: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
    for row in brk {
        by_msg
            .entry(row.get::<i64, _>("message_id"))
            .or_default()
            .push((row.get::<String, _>("emoji"), row.get::<i64, _>("c")));
    }
    for h in &mut highlights {
        if let Some(e) = by_msg.remove(&h.message_id) {
            h.emojis = e;
        }
    }
    Ok(highlights)
}
