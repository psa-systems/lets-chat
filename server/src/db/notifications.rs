use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// Per-room notification preference. Absence of a row in
/// `room_notification_settings` is treated as `MuteMode::None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteMode {
    None,
    ExceptMentions,
    All,
}

impl MuteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MuteMode::None => "none",
            MuteMode::ExceptMentions => "except_mentions",
            MuteMode::All => "all",
        }
    }

    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "none" => Some(MuteMode::None),
            "except_mentions" => Some(MuteMode::ExceptMentions),
            "all" => Some(MuteMode::All),
            _ => None,
        }
    }

    /// Whether a room-kind `Mentioned` event should reach the recipient.
    /// `All` blocks even mentions; `None` and `ExceptMentions` allow them.
    /// DM-kind mentions bypass this check at the call site.
    pub fn allows_room_mention(self) -> bool {
        !matches!(self, MuteMode::All)
    }

    /// Whether a non-mention `NewMessage` should bump the recipient's
    /// sidebar unread badge. Only the unmuted state allows the bump; both
    /// `All` and `ExceptMentions` suppress it.
    pub fn allows_unread_bump(self) -> bool {
        matches!(self, MuteMode::None)
    }
}

/// Single-row lookup. Returns `MuteMode::None` if no row exists. Called from
/// the WS render path on every `Mentioned` / `NewMessage` fan-out targeted at
/// the recipient.
pub async fn room_mute_mode(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<MuteMode, sqlx::Error> {
    let row = sqlx::query(
        "SELECT mute_mode FROM room_notification_settings \
          WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => MuteMode::None,
        Some(r) => {
            let raw: String = r.get("mute_mode");
            MuteMode::parse_str(&raw).unwrap_or(MuteMode::None)
        }
    })
}

/// Bulk loader for sidebar rendering. Returns rooms where the user has a
/// setting; rooms without rows are absent from the map and the caller
/// substitutes `MuteMode::None`.
pub async fn room_mute_modes_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashMap<i64, MuteMode>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT room_id, mute_mode FROM room_notification_settings \
          WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let id: i64 = r.get("room_id");
            let raw: String = r.get("mute_mode");
            MuteMode::parse_str(&raw).map(|m| (id, m))
        })
        .collect())
}

/// Upsert the mute mode. `MuteMode::None` removes the row instead of writing
/// the literal `'none'` so absence-default stays the schema invariant - empty
/// table = nobody has muted anything.
pub async fn set_room_mute_mode(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    mode: MuteMode,
) -> Result<(), sqlx::Error> {
    if matches!(mode, MuteMode::None) {
        return delete_room_mute_setting(pool, user_id, room_id).await;
    }
    sqlx::query(
        "INSERT INTO room_notification_settings \
             (user_id, room_id, mute_mode, updated_at) \
         VALUES (?, ?, ?, datetime('now')) \
         ON CONFLICT(user_id, room_id) DO UPDATE SET \
             mute_mode = excluded.mute_mode, \
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(mode.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_room_mute_setting(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM room_notification_settings \
          WHERE user_id = ? AND room_id = ?",
    )
    .bind(user_id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}
