use std::collections::HashSet;

use sqlx::{Acquire, Row, SqlitePool};

/// Per-room cap on the number of pinned messages. The 51st pin attempt
/// returns `Err(sqlx::Error::Protocol("pin cap reached"))` so the route
/// layer can surface a 409.
pub const MAX_PINS_PER_ROOM: i64 = 50;

/// One pinned message with the fields needed to render the strip and the
/// full pin-list page. Author and pinner display fields are resolved by
/// the caller via a single bulk auth lookup.
pub struct PinnedRow {
    pub message_id: i64,
    pub room_id: i64,
    pub pinned_by_user_id: String,
    pub pinned_at: String,
    pub author_user_id: String,
    pub body: String,
}

/// Insert a pin row. Idempotent: pinning a message that is already pinned
/// is a no-op (the PRIMARY KEY on `message_id` would otherwise raise a
/// constraint violation). Cap is enforced inside a single transaction so
/// two parallel pins on the 50th slot cannot both win.
pub async fn pin_message(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    pinned_by: &str,
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;

    // Already pinned -> success, no-op.
    let already: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM pinned_messages WHERE message_id = ?")
            .bind(message_id)
            .fetch_optional(&mut *tx)
            .await?;
    if already.is_some() {
        tx.commit().await?;
        return Ok(());
    }

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(&mut *tx)
        .await?;
    if n >= MAX_PINS_PER_ROOM {
        return Err(sqlx::Error::Protocol("pin cap reached".into()));
    }

    sqlx::query(
        "INSERT INTO pinned_messages (message_id, room_id, pinned_by) VALUES (?, ?, ?)",
    )
    .bind(message_id)
    .bind(room_id)
    .bind(pinned_by)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Remove a pin row. Idempotent: unpinning a message that is not pinned
/// is a successful no-op.
pub async fn unpin_message(pool: &SqlitePool, message_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pinned_messages WHERE message_id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Newest-first list of pinned, non-deleted messages for a room. `limit`
/// caps the result so the header strip can ask for top-3 while the
/// full-list page asks for `MAX_PINS_PER_ROOM`. Filters out soft-deleted
/// messages so a stale pin row does not surface a tombstone.
pub async fn pins_for_room(
    pool: &SqlitePool,
    room_id: i64,
    limit: i64,
) -> Result<Vec<PinnedRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT pm.message_id, pm.room_id, pm.pinned_by, pm.pinned_at, \
                m.user_id AS author_user_id, m.body AS body \
           FROM pinned_messages pm \
           INNER JOIN messages m ON m.id = pm.message_id \
          WHERE pm.room_id = ? \
            AND m.deleted_at IS NULL \
          ORDER BY pm.pinned_at DESC \
          LIMIT ?",
    )
    .bind(room_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PinnedRow {
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            pinned_by_user_id: r.get("pinned_by"),
            pinned_at: r.get("pinned_at"),
            author_user_id: r.get("author_user_id"),
            body: r.get("body"),
        })
        .collect())
}

/// Count of pinned, non-deleted messages in a room. Drives the
/// "See all (N) pinned" copy in the header strip.
pub async fn count_for_room(pool: &SqlitePool, room_id: i64) -> Result<i64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pinned_messages pm \
           INNER JOIN messages m ON m.id = pm.message_id \
          WHERE pm.room_id = ? AND m.deleted_at IS NULL",
    )
    .bind(room_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Set of message ids currently pinned in a room (and not soft-deleted).
/// Used by the page render path to populate `MessageView::is_pinned` in
/// bulk without an N+1 lookup.
pub async fn pinned_message_ids_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT pm.message_id FROM pinned_messages pm \
           INNER JOIN messages m ON m.id = pm.message_id \
          WHERE pm.room_id = ? AND m.deleted_at IS NULL",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("message_id"))
        .collect())
}

/// Look up the room id for a message, ignoring soft-deleted messages.
/// `None` for nonexistent or deleted messages. The pin/unpin route
/// handlers use this to derive the room id from the URL message id
/// without trusting client input.
pub async fn room_for_message(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let opt: Option<i64> = sqlx::query_scalar(
        "SELECT room_id FROM messages WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(opt)
}
