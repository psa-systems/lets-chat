use sqlx::Row;

use crate::models::{Message, Room};

pub async fn list_rooms(pool: &sqlx::SqlitePool) -> Result<Vec<Room>, sqlx::Error> {
    let rows = sqlx::query("SELECT id, name, topic, created_at FROM rooms ORDER BY name")
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| Room {
            id: row.get("id"),
            name: row.get("name"),
            topic: row.get("topic"),
            created_at: row.get("created_at"),
        })
        .collect())
}

pub async fn get_room(pool: &sqlx::SqlitePool, room_id: i64) -> Result<Option<Room>, sqlx::Error> {
    let row = sqlx::query("SELECT id, name, topic, created_at FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|row| Room {
        id: row.get("id"),
        name: row.get("name"),
        topic: row.get("topic"),
        created_at: row.get("created_at"),
    }))
}

pub async fn list_messages(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, author, body, created_at \
         FROM messages WHERE room_id = ? ORDER BY id ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Message {
            id: row.get("id"),
            room_id: row.get("room_id"),
            author: row.get("author"),
            body: row.get("body"),
            created_at: row.get("created_at"),
        })
        .collect())
}

pub async fn insert_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    author: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO messages (room_id, author, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(author)
        .bind(body)
        .execute(pool)
        .await?;
    Ok(result.last_insert_rowid())
}
