//! LC-342: "shame tag" community-moderation prototype. Members of a
//! shame-tags-enabled enclave vote moderation tags onto a message; once a
//! hide-worthy tag passes the vote threshold within the aging window, the
//! message renders default-hidden behind a click-through. A moderator override
//! (force show / force hide) wins over the community decision.
//!
//! Prototype constants (taxonomy / threshold / decay) live here, not in config.

use std::collections::HashMap;

use sqlx::{Row, SqlitePool};

/// The moderation tags a member can apply. Lowercase-kebab.
pub const TAGS: &[&str] = &["spam", "abusive", "off-topic", "misinformation"];

/// Tags that, past the threshold, default-hide the message. The others
/// annotate only.
pub const HIDE_TAGS: &[&str] = &["spam", "abusive"];

/// Distinct-voter count (within the decay window) at which a hide-worthy tag
/// hides the message.
pub const HIDE_THRESHOLD: i64 = 3;

/// Votes older than this stop counting toward the hide threshold.
pub const DECAY_DAYS: i64 = 30;

pub fn is_valid_tag(tag: &str) -> bool {
    TAGS.contains(&tag)
}

/// Per-message hide decision for the room render.
#[derive(Debug, Clone, PartialEq)]
pub struct HiddenState {
    /// Why it is hidden: a tag name, or "moderator" for a force-hide override.
    pub reason: String,
    /// True when a moderator force-hid it (vs community vote threshold).
    pub by_moderator: bool,
}

/// Toggle the caller's vote on `(message_id, tag)`. Returns the new state:
/// `true` = now voted, `false` = vote removed.
pub async fn toggle_vote(
    pool: &SqlitePool,
    message_id: i64,
    tag: &str,
    voter_user_id: &str,
) -> Result<bool, sqlx::Error> {
    let deleted =
        sqlx::query("DELETE FROM message_tags WHERE message_id=? AND tag=? AND voter_user_id=?")
            .bind(message_id)
            .bind(tag)
            .bind(voter_user_id)
            .execute(pool)
            .await?
            .rows_affected();
    if deleted > 0 {
        return Ok(false);
    }
    sqlx::query("INSERT INTO message_tags (message_id, tag, voter_user_id) VALUES (?, ?, ?)")
        .bind(message_id)
        .bind(tag)
        .bind(voter_user_id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Distinct-voter counts per tag for one message, within the decay window.
/// Returns a map tag -> count (only tags with >=1 in-window vote).
pub async fn tag_counts(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<HashMap<String, i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT tag, COUNT(*) AS c FROM message_tags \
         WHERE message_id=? AND created_at > datetime('now', '-' || ? || ' days') \
         GROUP BY tag",
    )
    .bind(message_id)
    .bind(DECAY_DAYS)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("tag"), r.get::<i64, _>("c")))
        .collect())
}

/// The set of tags the caller has voted on this message (to highlight their
/// own votes in the control).
pub async fn voter_tags(
    pool: &SqlitePool,
    message_id: i64,
    voter_user_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT tag FROM message_tags WHERE message_id=? AND voter_user_id=?")
        .bind(message_id)
        .bind(voter_user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("tag"))
        .collect())
}

/// Set (or replace) a moderator visibility override. `hidden=true` force-hides,
/// `false` force-shows; either wins over the vote threshold.
pub async fn set_override(
    pool: &SqlitePool,
    message_id: i64,
    hidden: bool,
    actor_user: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_tag_overrides (message_id, hidden, actor_user) VALUES (?, ?, ?) \
         ON CONFLICT(message_id) DO UPDATE SET hidden=excluded.hidden, actor_user=excluded.actor_user, created_at=datetime('now')",
    )
    .bind(message_id)
    .bind(if hidden { 1_i64 } else { 0_i64 })
    .bind(actor_user)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a moderator override (revert to the community decision).
pub async fn clear_override(pool: &SqlitePool, message_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM message_tag_overrides WHERE message_id=?")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The current moderator override for a message: `Some(true)` force-hidden,
/// `Some(false)` force-shown, `None` no override.
pub async fn get_override(pool: &SqlitePool, message_id: i64) -> Result<Option<bool>, sqlx::Error> {
    let row = sqlx::query("SELECT hidden FROM message_tag_overrides WHERE message_id=?")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("hidden") != 0))
}

/// The hide decision for a single message (override-aware). `None` = visible.
pub async fn hidden_state(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<Option<HiddenState>, sqlx::Error> {
    let states = hidden_states_for_messages(pool, &[message_id]).await?;
    Ok(states.into_values().next())
}

/// Batch hide decisions for the room render: one override query + one
/// vote-threshold query over all `message_ids`. Returns only the hidden ones.
/// Override wins (force-show suppresses any vote-hide; force-hide always hides).
pub async fn hidden_states_for_messages(
    pool: &SqlitePool,
    message_ids: &[i64],
) -> Result<HashMap<i64, HiddenState>, sqlx::Error> {
    let mut out: HashMap<i64, HiddenState> = HashMap::new();
    if message_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = vec!["?"; message_ids.len()].join(", ");

    // Moderator overrides first: a force-show (hidden=0) pins the message
    // visible regardless of votes; a force-hide (hidden=1) pins it hidden.
    let mut forced_visible: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let sql = format!(
        "SELECT message_id, hidden FROM message_tag_overrides WHERE message_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.get("message_id");
        if r.get::<i64, _>("hidden") != 0 {
            out.insert(
                id,
                HiddenState {
                    reason: "moderator".to_string(),
                    by_moderator: true,
                },
            );
        } else {
            forced_visible.insert(id);
        }
    }

    // Community vote-hide: a hide-worthy tag past threshold within the window.
    // Skip any message already decided by an override.
    let tag_ph = vec!["?"; HIDE_TAGS.len()].join(", ");
    let sql = format!(
        "SELECT message_id, tag, COUNT(*) AS c FROM message_tags \
         WHERE message_id IN ({placeholders}) AND tag IN ({tag_ph}) \
           AND created_at > datetime('now', '-' || ? || ' days') \
         GROUP BY message_id, tag HAVING c >= ?"
    );
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    for t in HIDE_TAGS {
        q = q.bind(t);
    }
    q = q.bind(DECAY_DAYS).bind(HIDE_THRESHOLD);
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.get("message_id");
        if forced_visible.contains(&id) || out.contains_key(&id) {
            continue;
        }
        out.insert(
            id,
            HiddenState {
                reason: r.get::<String, _>("tag"),
                by_moderator: false,
            },
        );
    }
    Ok(out)
}
