//! LC-1059: per-enclave welcome-message config (`chat.db::enclave_welcome`).

use sqlx::{Row, SqlitePool};

/// Welcome config for one enclave. `room_id = None` means the default room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WelcomeConfig {
    pub enabled: bool,
    pub room_id: Option<i64>,
}

impl Default for WelcomeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            room_id: None,
        }
    }
}

/// Read the config; a missing row is the default (enabled, default room).
pub async fn get(pool: &SqlitePool, enclave_id: i64) -> Result<WelcomeConfig, sqlx::Error> {
    let row = sqlx::query("SELECT enabled, room_id FROM enclave_welcome WHERE enclave_id = ?")
        .bind(enclave_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map_or_else(WelcomeConfig::default, |r| WelcomeConfig {
        enabled: r.get::<i64, _>("enabled") != 0,
        room_id: r.get("room_id"),
    }))
}

pub async fn set(
    pool: &SqlitePool,
    enclave_id: i64,
    cfg: WelcomeConfig,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO enclave_welcome (enclave_id, enabled, room_id) VALUES (?, ?, ?) \
         ON CONFLICT(enclave_id) DO UPDATE SET enabled = excluded.enabled, room_id = excluded.room_id",
    )
    .bind(enclave_id)
    .bind(i64::from(cfg.enabled))
    .bind(cfg.room_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Public, non-voice rooms of the enclave that can carry a welcome, by name.
pub async fn eligible_rooms(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name FROM rooms \
         WHERE enclave_id = ? AND room_type = 'public' AND is_voice = 0 \
         ORDER BY name",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| (r.get("id"), r.get("name"))).collect())
}

/// The room a welcome posts to, or `None` when disabled / no eligible room.
/// A configured room that is no longer eligible (deleted, made private or
/// voice, moved) falls back to the default (first eligible room by name).
pub async fn resolve_room(pool: &SqlitePool, enclave_id: i64) -> Result<Option<i64>, sqlx::Error> {
    let cfg = get(pool, enclave_id).await?;
    if !cfg.enabled {
        return Ok(None);
    }
    let rooms = eligible_rooms(pool, enclave_id).await?;
    Ok(cfg
        .room_id
        .filter(|rid| rooms.iter().any(|(id, _)| id == rid))
        .or_else(|| rooms.first().map(|(id, _)| *id)))
}

/// Welcome lines, rendered after the newcomer's name ("**alice** just showed
/// up!"). Stored verbatim as the system message body, so `is_welcome` on a
/// system row is a body match against this list. Renaming a line drops the
/// wave button from old messages that used it.
pub const WELCOME_LINES: &[&str] = &[
    "just showed up!",
    "hopped into the enclave.",
    "joined the party.",
    "is here. Say hi!",
    "just landed.",
    "arrived. Hope you brought snacks.",
    "just slid into the enclave.",
    "is here to hang out.",
    "appeared. Everyone look busy!",
    "joined. Welcome aboard!",
];

pub fn is_welcome_line(body: &str) -> bool {
    WELCOME_LINES.contains(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_lines_are_distinct_from_call_events() {
        let mut seen = std::collections::HashSet::new();
        for line in WELCOME_LINES {
            assert!(!line.trim().is_empty());
            assert!(seen.insert(*line), "duplicate welcome line {line:?}");
        }
        // Call-event bodies must not read as welcomes (they group, welcomes don't).
        assert!(!is_welcome_line("started a call."));
        assert!(!is_welcome_line("declined the call."));
        assert!(is_welcome_line(WELCOME_LINES[0]));
    }
}
