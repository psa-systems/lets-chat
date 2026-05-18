//! Unread-message inbox (LC-81).
//!
//! Flattens unread messages across every room the user has access to
//! into a single newest-first timeline. Reuses the existing
//! `dm_read_state` watermark table (room-keyed; works for both DMs
//! and channels) and the same accessibility predicate the sidebar
//! unread-count queries use.
use sqlx::{Row, SqlitePool};

pub struct InboxRow {
    pub message_id: i64,
    pub room_id: i64,
    pub room_name: String,
    pub room_type: String,
    pub author_user_id: String,
    pub body: String,
    pub created_at: String,
}

/// Page through unread messages newest-first. `before_id = None`
/// fetches the first page; subsequent calls pass the smallest
/// message_id from the previous page as the cursor. Limit is
/// inclusive (server returns at most `limit` rows).
pub async fn list_unread(
    pool: &SqlitePool,
    user_id: &str,
    is_admin: bool,
    limit: i64,
    before_id: Option<i64>,
) -> Result<Vec<InboxRow>, sqlx::Error> {
    // Admin sees every public + private room; non-admins see public
    // rooms always, plus private rooms / DMs they are a member of.
    let access_clause = if is_admin {
        // No additional membership filter; rooms.room_type != 'dm'
        // would let admins see channel content but skip private DMs.
        // We still want admins to see their own unread DMs, so allow
        // DM access only via membership.
        "(r.room_type = 'public' OR r.room_type != 'dm' \
          OR EXISTS (SELECT 1 FROM room_members rm WHERE rm.room_id = r.id AND rm.user_id = ?))"
    } else {
        "(r.room_type = 'public' \
          OR EXISTS (SELECT 1 FROM room_members rm WHERE rm.room_id = r.id AND rm.user_id = ?))"
    };
    let cursor_clause = if before_id.is_some() {
        "AND m.id < ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT m.id AS message_id, m.room_id, m.user_id AS author_user_id, \
                m.body, m.created_at, r.name AS room_name, r.room_type \
           FROM messages m \
           JOIN rooms r ON r.id = m.room_id \
           LEFT JOIN dm_read_state s ON s.room_id = m.room_id AND s.user_id = ? \
          WHERE m.user_id != ? \
            AND m.deleted_at IS NULL \
            AND m.parent_id IS NULL \
            AND m.id > COALESCE(s.last_read_message_id, 0) \
            AND {access_clause} \
            {cursor_clause} \
          ORDER BY m.created_at DESC, m.id DESC \
          LIMIT ?"
    );
    let mut q = sqlx::query(&sql).bind(user_id).bind(user_id).bind(user_id);
    if let Some(id) = before_id {
        q = q.bind(id);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| InboxRow {
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            room_type: r.get("room_type"),
            author_user_id: r.get("author_user_id"),
            body: r.get("body"),
            created_at: r.get("created_at"),
        })
        .collect())
}
