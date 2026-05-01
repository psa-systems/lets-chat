use sqlx::Row;

use crate::models::{Reaction, Room, SearchResult};

/// Raw message row from the chat DB — contains user_id but no author_name.
/// The server fn layer resolves the display name from the auth DB.
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
}

fn map_room(row: &sqlx::sqlite::SqliteRow) -> Room {
    Room {
        id: row.get("id"),
        name: row.get("name"),
        topic: row.get("topic"),
        room_type: row.get("room_type"),
        invite_code: row.get("invite_code"),
        created_at: row.get("created_at"),
    }
}

/// List rooms visible to a user.
/// Admins see all non-DM rooms. Regular users see public rooms plus private rooms they joined.
pub async fn list_rooms(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<Room>, sqlx::Error> {
    if is_admin {
        let rows = sqlx::query(
            "SELECT id, name, topic, room_type, invite_code, created_at \
             FROM rooms WHERE room_type != 'dm' ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        return Ok(rows.iter().map(map_room).collect());
    }

    let rows = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at \
         FROM rooms r \
         LEFT JOIN room_members m ON m.room_id = r.id AND m.user_id = ? \
         WHERE r.room_type != 'dm' AND (r.room_type = 'public' OR m.user_id IS NOT NULL) \
         ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(map_room).collect())
}

pub async fn get_room(pool: &sqlx::SqlitePool, room_id: i64) -> Result<Option<Room>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, topic, room_type, invite_code, created_at FROM rooms WHERE id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(map_room))
}

pub async fn list_messages(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages WHERE room_id = ? AND deleted_at IS NULL ORDER BY id ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| RawMessage {
            id: row.get("id"),
            room_id: row.get("room_id"),
            user_id: row.get("user_id"),
            body: row.get("body"),
            created_at: row.get("created_at"),
            edited_at: row.get("edited_at"),
        })
        .collect())
}

/// Fetch a single message by ID. Returns None if soft-deleted.
pub async fn get_message(
    pool: &sqlx::SqlitePool,
    message_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at \
         FROM messages WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| RawMessage {
        id: row.get("id"),
        room_id: row.get("room_id"),
        user_id: row.get("user_id"),
        body: row.get("body"),
        created_at: row.get("created_at"),
        edited_at: row.get("edited_at"),
    }))
}

/// Update a message's body and set edited_at to now. Returns the edited_at timestamp.
pub async fn update_message_body(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    new_body: &str,
) -> Result<String, sqlx::Error> {
    let edited_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query("UPDATE messages SET body = ?, edited_at = ? WHERE id = ?")
        .bind(new_body)
        .bind(&edited_at)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(edited_at)
}

pub async fn insert_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}

pub async fn create_room(
    pool: &sqlx::SqlitePool,
    name: &str,
    topic: Option<&str>,
    room_type: &str,
    invite_code: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let result =
        sqlx::query("INSERT INTO rooms (name, topic, room_type, invite_code) VALUES (?, ?, ?, ?)")
            .bind(name)
            .bind(topic)
            .bind(room_type)
            .bind(invite_code)
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

pub async fn delete_room(pool: &sqlx::SqlitePool, room_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    name: &str,
    topic: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE rooms SET name = ?, topic = ? WHERE id = ?")
        .bind(name)
        .bind(topic)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Check if a user is a member of a room.
pub async fn is_room_member(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM room_members WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Add a user to a room's member list. No-op if already a member (INSERT OR IGNORE).
pub async fn add_room_member(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(room_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a user from a room's member list.
pub async fn remove_room_member(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM room_members WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Count members in a room (used by the admin rooms table).
pub async fn count_room_members(pool: &sqlx::SqlitePool, room_id: i64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM room_members WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get("c"))
}

/// Find a room by its invite code.
pub async fn get_room_by_invite(
    pool: &sqlx::SqlitePool,
    invite_code: &str,
) -> Result<Option<Room>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, topic, room_type, invite_code, created_at \
         FROM rooms WHERE invite_code = ?",
    )
    .bind(invite_code)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_room))
}

/// Update the invite code for a room.
pub async fn regenerate_invite_code(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    new_code: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE rooms SET invite_code = ? WHERE id = ?")
        .bind(new_code)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Find an existing DM room between two users.
pub async fn find_dm_room(
    pool: &sqlx::SqlitePool,
    user_a: &str,
    user_b: &str,
) -> Result<Option<Room>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at \
         FROM rooms r \
         JOIN room_members m1 ON m1.room_id = r.id AND m1.user_id = ? \
         JOIN room_members m2 ON m2.room_id = r.id AND m2.user_id = ? \
         WHERE r.room_type = 'dm'",
    )
    .bind(user_a)
    .bind(user_b)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(map_room))
}

/// Create a DM room between two users.
pub async fn create_dm_room(
    pool: &sqlx::SqlitePool,
    name: &str,
    user_a: &str,
    user_b: &str,
) -> Result<Room, sqlx::Error> {
    let result = sqlx::query("INSERT INTO rooms (name, room_type, created_by) VALUES (?, 'dm', ?)")
        .bind(name)
        .bind(user_a)
        .execute(pool)
        .await?;
    let room_id = result.last_insert_rowid();

    sqlx::query("INSERT INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(room_id)
        .bind(user_a)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO room_members (room_id, user_id) VALUES (?, ?)")
        .bind(room_id)
        .bind(user_b)
        .execute(pool)
        .await?;

    get_room(pool, room_id)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)
}

/// List DM rooms for a user, returning Room + the other user's ID.
pub async fn list_user_dm_rooms(
    pool: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Vec<(Room, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, m2.user_id as other_user \
         FROM rooms r \
         JOIN room_members m1 ON m1.room_id = r.id AND m1.user_id = ? \
         JOIN room_members m2 ON m2.room_id = r.id AND m2.user_id != ? \
         WHERE r.room_type = 'dm' \
         ORDER BY r.created_at DESC",
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let room = map_room(&row);
            let other: String = row.get("other_user");
            (room, other)
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct DmReadState {
    pub user_id: String,
    pub room_id: i64,
    pub last_read_message_id: i64,
    pub updated_at: String,
}

/// Upsert the caller's last-read watermark for a DM. Monotonic: never decreases.
pub async fn upsert_dm_read(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
    message_id: i64,
) -> Result<String, sqlx::Error> {
    let updated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO dm_read_state (user_id, room_id, last_read_message_id, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, room_id) DO UPDATE SET \
           last_read_message_id = MAX(excluded.last_read_message_id, dm_read_state.last_read_message_id), \
           updated_at = CASE \
             WHEN excluded.last_read_message_id > dm_read_state.last_read_message_id \
             THEN excluded.updated_at ELSE dm_read_state.updated_at END",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(message_id)
    .bind(&updated_at)
    .execute(pool)
    .await?;
    Ok(updated_at)
}

pub async fn get_dm_read_state(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<Option<DmReadState>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT user_id, room_id, last_read_message_id, updated_at \
         FROM dm_read_state WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| DmReadState {
        user_id: r.get("user_id"),
        room_id: r.get("room_id"),
        last_read_message_id: r.get("last_read_message_id"),
        updated_at: r.get("updated_at"),
    }))
}

/// For each DM the user is a member of, count peer messages newer than the user's watermark.
pub async fn list_dm_unread_counts(
    pool: &sqlx::SqlitePool,
    user_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT r.id AS room_id, \
                COUNT(m.id) AS unread \
         FROM rooms r \
         JOIN room_members rm ON rm.room_id = r.id AND rm.user_id = ? \
         LEFT JOIN dm_read_state s ON s.room_id = r.id AND s.user_id = ? \
         LEFT JOIN messages m \
           ON m.room_id = r.id \
          AND m.user_id != ? \
          AND m.deleted_at IS NULL \
          AND m.id > COALESCE(s.last_read_message_id, 0) \
         WHERE r.room_type = 'dm' \
         GROUP BY r.id",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("room_id"), r.get::<i64, _>("unread")))
        .collect())
}

/// For each non-DM room visible to the user (public rooms plus private rooms
/// they joined), count messages from other users that are newer than the
/// caller's watermark in dm_read_state. The dm_read_state table is room-keyed,
/// so we reuse it for non-DM rooms as well.
pub async fn list_room_unread_counts(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    // Admins see every non-DM room regardless of membership.
    let sql = if is_admin {
        "SELECT r.id AS room_id, \
                COUNT(m.id) AS unread \
         FROM rooms r \
         LEFT JOIN dm_read_state s ON s.room_id = r.id AND s.user_id = ? \
         LEFT JOIN messages m \
           ON m.room_id = r.id \
          AND m.user_id != ? \
          AND m.deleted_at IS NULL \
          AND m.id > COALESCE(s.last_read_message_id, 0) \
         WHERE r.room_type != 'dm' \
         GROUP BY r.id"
    } else {
        "SELECT r.id AS room_id, \
                COUNT(m.id) AS unread \
         FROM rooms r \
         LEFT JOIN room_members rm ON rm.room_id = r.id AND rm.user_id = ? \
         LEFT JOIN dm_read_state s ON s.room_id = r.id AND s.user_id = ? \
         LEFT JOIN messages m \
           ON m.room_id = r.id \
          AND m.user_id != ? \
          AND m.deleted_at IS NULL \
          AND m.id > COALESCE(s.last_read_message_id, 0) \
         WHERE r.room_type != 'dm' \
           AND (r.room_type = 'public' OR rm.user_id IS NOT NULL) \
         GROUP BY r.id"
    };

    let mut q = sqlx::query(sql).bind(user_id);
    if !is_admin {
        q = q.bind(user_id);
    }
    q = q.bind(user_id);

    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("room_id"), r.get::<i64, _>("unread")))
        .collect())
}

/// Set the caller's last-read watermark for any room (DM or non-DM). Wraps
/// upsert_dm_read since dm_read_state is room-keyed and not constrained to
/// DM rooms; the name is preserved for backward compatibility, but the table
/// is used as a generic per-user, per-room watermark.
pub async fn set_last_read(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
    message_id: i64,
) -> Result<String, sqlx::Error> {
    upsert_dm_read(pool, user_id, room_id, message_id).await
}

/// Return the peer's user_id for a DM room from the caller's perspective, or
/// None if the room is not a DM the caller is a member of.
pub async fn get_dm_peer(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT m2.user_id AS peer \
         FROM rooms r \
         JOIN room_members m1 ON m1.room_id = r.id AND m1.user_id = ? \
         JOIN room_members m2 ON m2.room_id = r.id AND m2.user_id != ? \
         WHERE r.id = ? AND r.room_type = 'dm'",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<String, _>("peer")))
}

// ── Reactions ─────────────────────────────────────────────────────────────────

/// Toggle a reaction: insert if not present, delete if already present.
/// Returns `true` if the reaction was added, `false` if it was removed.
pub async fn toggle_reaction(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    user_id: &str,
    emoji: &str,
) -> Result<bool, sqlx::Error> {
    let existing = sqlx::query(
        "SELECT 1 FROM message_reactions WHERE message_id = ? AND user_id = ? AND emoji = ?",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .fetch_optional(pool)
    .await?;

    if existing.is_some() {
        sqlx::query(
            "DELETE FROM message_reactions WHERE message_id = ? AND user_id = ? AND emoji = ?",
        )
        .bind(message_id)
        .bind(user_id)
        .bind(emoji)
        .execute(pool)
        .await?;
        Ok(false)
    } else {
        sqlx::query("INSERT INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)")
            .bind(message_id)
            .bind(user_id)
            .bind(emoji)
            .execute(pool)
            .await?;
        Ok(true)
    }
}

/// List reactions grouped by emoji for a message.
/// `caller_user_id` is used to populate `reacted_by_me`.
pub async fn list_reactions(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    caller_user_id: &str,
) -> Result<Vec<Reaction>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT emoji, COUNT(*) AS count, \
                MAX(CASE WHEN user_id = ? THEN 1 ELSE 0 END) AS reacted_by_me \
         FROM message_reactions \
         WHERE message_id = ? \
         GROUP BY emoji \
         ORDER BY MIN(created_at)",
    )
    .bind(caller_user_id)
    .bind(message_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Reaction {
            emoji: r.get("emoji"),
            count: r.get("count"),
            reacted_by_me: r.get::<i64, _>("reacted_by_me") == 1,
        })
        .collect())
}

/// List all reactions for every message in a room, in a single query.
/// Returns a vec of `(message_id, Reaction)` for messages that have at least one reaction.
pub async fn list_room_reactions(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    caller_user_id: &str,
) -> Result<Vec<(i64, Reaction)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT mr.message_id, mr.emoji, \
                COUNT(*) AS count, \
                MAX(CASE WHEN mr.user_id = ? THEN 1 ELSE 0 END) AS reacted_by_me \
         FROM message_reactions mr \
         JOIN messages m ON m.id = mr.message_id \
         WHERE m.room_id = ? AND m.deleted_at IS NULL \
         GROUP BY mr.message_id, mr.emoji \
         ORDER BY mr.message_id, MIN(mr.created_at)",
    )
    .bind(caller_user_id)
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("message_id"),
                Reaction {
                    emoji: r.get("emoji"),
                    count: r.get("count"),
                    reacted_by_me: r.get::<i64, _>("reacted_by_me") == 1,
                },
            )
        })
        .collect())
}

/// Escape a raw user query string for safe use in an FTS5 MATCH expression.
/// Splits on whitespace, strips FTS5 special characters from each token,
/// and drops empty tokens. Returns None if no usable tokens remain.
pub fn sanitize_fts_query(raw: &str) -> Option<String> {
    let special = |c: char| matches!(c, '"' | '*' | '(' | ')' | '+' | '-' | '^' | ':');
    let tokens: Vec<String> = raw
        .split_whitespace()
        .map(|t| t.replace(special, ""))
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        None
    } else {
        // Wrap each token in double-quotes so FTS5 treats it as a literal term
        // rather than a command keyword.
        Some(
            tokens
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// Full-text search across accessible messages.
///
/// For non-admin callers the result is restricted to public rooms plus rooms
/// the caller is a member of.  Pass `is_admin = true` to skip the access
/// filter.  `room_id_filter` narrows results to a single room when set.
pub async fn search_messages(
    pool: &sqlx::SqlitePool,
    fts_query: &str,
    room_id_filter: Option<i64>,
    caller_user_id: &str,
    is_admin: bool,
) -> Result<Vec<SearchResult>, sqlx::Error> {
    // Build the room-access predicate inline so we can use a single query.
    // Admin: no restriction.
    // Regular user: public rooms OR rooms where caller is a member.
    let room_filter_clause = match room_id_filter {
        Some(_) => "AND m.room_id = ?",
        None => "",
    };

    let access_clause = if is_admin {
        "AND r.room_type != 'dm'"
    } else {
        "AND (r.room_type = 'public' OR EXISTS (\
             SELECT 1 FROM room_members rm \
             WHERE rm.room_id = r.id AND rm.user_id = ?))"
    };

    let sql = format!(
        "SELECT m.id AS message_id, m.room_id, r.name AS room_name, \
                m.body, m.user_id, m.created_at \
         FROM messages_fts \
         JOIN messages m ON m.id = messages_fts.rowid \
         JOIN rooms    r ON r.id  = m.room_id \
         WHERE messages_fts MATCH ? \
           AND m.deleted_at IS NULL \
           {room_filter_clause} \
           {access_clause} \
         ORDER BY messages_fts.rank \
         LIMIT 50"
    );

    // Bind parameters in order: fts_query, [room_id_filter], [caller_user_id]
    let mut q = sqlx::query(&sql).bind(fts_query);
    if let Some(rid) = room_id_filter {
        q = q.bind(rid);
    }
    if !is_admin {
        q = q.bind(caller_user_id);
    }

    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|r| SearchResult {
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            body: r.get("body"),
            user_id: r.get("user_id"),
            author_name: r.get::<String, _>("user_id"), // resolved by server fn layer
            created_at: r.get("created_at"),
        })
        .collect())
}
