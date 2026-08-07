//! LC-536: personal member stats ("wrapped"). Read-only aggregates over the
//! caller's own activity in `chat.db` (messages, reactions, kudos). Everything
//! here is scoped to a single `user_id` and surfaced only to that user, so no
//! cross-user privacy gate is needed. Deleted / system / quarantined messages
//! are excluded so the counts match what the user (and others) actually see.

use sqlx::{Row, SqlitePool};

/// A single "top channel" tally: the room name and how many of the user's
/// messages landed there.
pub struct TopRoom {
    pub name: String,
    pub count: i64,
}

/// The user's personal recap. All fields are all-time totals.
pub struct MemberStats {
    pub messages_sent: i64,
    /// Distinct calendar days (UTC) on which the user posted a message.
    pub active_days: i64,
    pub reactions_given: i64,
    /// Reactions others placed on the user's messages (self-reactions excluded).
    pub reactions_received: i64,
    pub kudos_received: i64,
    /// Up to five channels the user posted in most, DMs excluded.
    pub top_rooms: Vec<TopRoom>,
}

/// Shared predicate: a message that counts toward a user's stats. Kept in one
/// place so `messages_sent`, `active_days`, and `top_rooms` stay consistent.
const COUNTABLE_MSG: &str = "deleted_at IS NULL AND is_system = 0 AND quarantined = 0";

async fn scalar_count(pool: &SqlitePool, sql: &str, user_id: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(sql).bind(user_id).fetch_one(pool).await?;
    Ok(row.get::<i64, _>(0))
}

/// LC-671: the user's message count over the past 7 days (same countable-message
/// predicate as the all-time stats). Feeds the weekly recap DM.
pub async fn weekly_message_count(pool: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    scalar_count(
        pool,
        &format!(
            "SELECT COUNT(*) FROM messages \
             WHERE user_id = ? AND {COUNTABLE_MSG} AND created_at >= datetime('now', '-7 days')"
        ),
        user_id,
    )
    .await
}

/// LC-671: kudos the user received over the past 7 days.
pub async fn weekly_kudos_received(pool: &SqlitePool, user_id: &str) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) FROM kudos \
         WHERE receiver_id = ? AND created_at >= datetime('now', '-7 days')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get::<i64, _>(0))
}

/// Aggregate the caller's personal stats in a handful of scoped queries.
pub async fn member_stats(pool: &SqlitePool, user_id: &str) -> Result<MemberStats, sqlx::Error> {
    let messages_sent = scalar_count(
        pool,
        &format!("SELECT COUNT(*) FROM messages WHERE user_id = ? AND {COUNTABLE_MSG}"),
        user_id,
    )
    .await?;

    let active_days = scalar_count(
        pool,
        &format!(
            "SELECT COUNT(DISTINCT date(created_at)) FROM messages \
             WHERE user_id = ? AND {COUNTABLE_MSG}"
        ),
        user_id,
    )
    .await?;

    let reactions_given = scalar_count(
        pool,
        "SELECT COUNT(*) FROM message_reactions WHERE user_id = ?",
        user_id,
    )
    .await?;

    // Reactions on the user's own messages, placed by someone else. Bind the id
    // twice: once for the message author, once to exclude self-reactions.
    let reactions_received = {
        let row = sqlx::query(
            "SELECT COUNT(*) FROM message_reactions r \
             JOIN messages m ON m.id = r.message_id \
             WHERE m.user_id = ? AND r.user_id != ?",
        )
        .bind(user_id)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        row.get::<i64, _>(0)
    };

    let kudos_received = scalar_count(
        pool,
        "SELECT COUNT(*) FROM kudos WHERE receiver_id = ?",
        user_id,
    )
    .await?;

    let top_rooms = sqlx::query(&format!(
        "SELECT r.name AS name, COUNT(*) AS c \
             FROM messages m JOIN rooms r ON r.id = m.room_id \
             WHERE m.user_id = ? AND m.{COUNTABLE_MSG} AND r.room_type != 'dm' \
             GROUP BY m.room_id ORDER BY c DESC, r.name LIMIT 5"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| TopRoom {
        name: row.get("name"),
        count: row.get("c"),
    })
    .collect();

    Ok(MemberStats {
        messages_sent,
        active_days,
        reactions_given,
        reactions_received,
        kudos_received,
        top_rooms,
    })
}
