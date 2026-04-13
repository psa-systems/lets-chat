use sqlx::Row;

use crate::models::Room;

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
    let result = sqlx::query(
        "INSERT INTO rooms (name, topic, room_type, invite_code) VALUES (?, ?, ?, ?)",
    )
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
    let row = sqlx::query(
        "SELECT 1 FROM room_members WHERE room_id = ? AND user_id = ?",
    )
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
    sqlx::query(
        "INSERT OR IGNORE INTO room_members (room_id, user_id) VALUES (?, ?)",
    )
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
    let result = sqlx::query(
        "INSERT INTO rooms (name, room_type, created_by) VALUES (?, 'dm', ?)",
    )
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
