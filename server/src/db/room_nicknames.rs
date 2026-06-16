//! LC-321: per-room nicknames. A user's self-set display name scoped to one
//! room, folded into the author label at `routes::load_author_meta` so every
//! message render in that room shows the nickname in place of the global
//! display_name / username. `user_id` references auth.db, so there is no
//! cross-db FK (same as `room_members`).
use std::collections::HashMap;

use sqlx::{Row, SqlitePool};

/// Max nickname length, enforced on the write path. Shorter than the global
/// display_name cap (64) - a contextual nickname is meant to be terse.
pub const MAX_ROOM_NICKNAME_CHARS: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum SetNicknameError {
    #[error("nickname exceeds {0} characters")]
    TooLong(usize),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Upsert a user's nickname for a room. Trims first; the caller is expected to
/// pass a non-empty value (use `clear` to remove). Rejects > 32 chars.
pub async fn set(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    nickname: &str,
) -> Result<(), SetNicknameError> {
    if nickname.chars().count() > MAX_ROOM_NICKNAME_CHARS {
        return Err(SetNicknameError::TooLong(MAX_ROOM_NICKNAME_CHARS));
    }
    sqlx::query(
        "INSERT INTO room_nicknames (room_id, user_id, nickname) VALUES (?, ?, ?) \
         ON CONFLICT(room_id, user_id) DO UPDATE SET nickname = excluded.nickname, \
         updated_at = datetime('now')",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(nickname)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a user's nickname for a room (revert to their global name). No-op if
/// none is set.
pub async fn clear(pool: &SqlitePool, room_id: i64, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM room_nicknames WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A user's nickname in a room, if set.
pub async fn get(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT nickname FROM room_nicknames WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<String, _>("nickname")))
}

/// All nicknames set in a room, keyed by user_id. For any future bulk-render
/// surface (e.g. a per-room member list); the message-label override path goes
/// through `get` via the per-author cache.
pub async fn for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id, nickname FROM room_nicknames WHERE room_id = ?")
        .bind(room_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("user_id"),
                r.get::<String, _>("nickname"),
            )
        })
        .collect())
}
