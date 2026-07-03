//! LC-526: kudos / recognition. Records giver/receiver/reason (see the
//! `/kudos` slash command in `routes::slash`) and aggregates the per-enclave
//! leaderboard. Additive only - there is no way to remove or downvote a kudos
//! through this module.

use sqlx::{Row, SqlitePool};

/// One leaderboard entry: a user id and their kudos count in the window.
#[derive(Debug, Clone)]
pub struct Leader {
    pub user_id: String,
    pub count: i64,
}

/// Record one kudos. `enclave_id` is the room's enclave (None for a non-enclave
/// room). Returns the new row id.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    pool: &SqlitePool,
    giver_id: &str,
    receiver_id: &str,
    room_id: i64,
    enclave_id: Option<i64>,
    reason: Option<&str>,
    message_id: Option<i64>,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO kudos (giver_id, receiver_id, room_id, enclave_id, reason, message_id) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(giver_id)
    .bind(receiver_id)
    .bind(room_id)
    .bind(enclave_id)
    .bind(reason)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Top kudos receivers within `enclave_ids` since `window` (a SQLite datetime
/// modifier like `"-30 days"`). Empty `enclave_ids` yields no rows.
pub async fn top_receivers(
    pool: &SqlitePool,
    enclave_ids: &[i64],
    window: &str,
    limit: i64,
) -> sqlx::Result<Vec<Leader>> {
    leaderboard(pool, "receiver_id", enclave_ids, window, limit).await
}

/// Top kudos givers, mirror of [`top_receivers`].
pub async fn top_givers(
    pool: &SqlitePool,
    enclave_ids: &[i64],
    window: &str,
    limit: i64,
) -> sqlx::Result<Vec<Leader>> {
    leaderboard(pool, "giver_id", enclave_ids, window, limit).await
}

/// Shared aggregate over a giver/receiver column. `column` is a fixed internal
/// identifier (never user input), so interpolating it is safe.
async fn leaderboard(
    pool: &SqlitePool,
    column: &str,
    enclave_ids: &[i64],
    window: &str,
    limit: i64,
) -> sqlx::Result<Vec<Leader>> {
    if enclave_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", enclave_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {column} AS uid, COUNT(*) AS c FROM kudos \
         WHERE enclave_id IN ({placeholders}) AND created_at >= datetime('now', ?) \
         GROUP BY {column} ORDER BY c DESC, uid LIMIT ?",
    );
    let mut q = sqlx::query(&sql);
    for id in enclave_ids {
        q = q.bind(id);
    }
    q = q.bind(window).bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| Leader {
            user_id: r.get::<String, _>("uid"),
            count: r.get::<i64, _>("c"),
        })
        .collect())
}
