//! LC-66: polls / voting. A poll is anchored to a `messages` row (the body
//! holds the question); `poll_options` + `poll_votes` carry the choices and
//! tallies. Voting mirrors reactions: toggle a row, then broadcast a
//! re-rendered fragment to the room.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct Poll {
    pub message_id: i64,
    pub question: String,
    pub allows_multi: bool,
    pub anonymous: bool,
    pub closes_at: Option<String>,
    pub closed_at: Option<String>,
    /// LC-491: when set, this poll is an event; `event_at` is the start time
    /// (UTC `%Y-%m-%d %H:%M:%S`) and the options are the RSVP choices.
    pub event_at: Option<String>,
    pub event_location: Option<String>,
}

/// One option row plus its current vote count.
#[derive(Debug, Clone)]
pub struct OptionTally {
    pub id: i64,
    pub position: i64,
    pub text: String,
    pub count: i64,
}

/// Create a poll: insert the anchor message, the poll row, and the options
/// in a single transaction (atomic per the acceptance criteria). Returns
/// the new message id. The body is the question so message search / quote /
/// pin keep working.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    question: &str,
    options: &[String],
    allows_multi: bool,
    anonymous: bool,
    closes_at: Option<&str>,
) -> sqlx::Result<i64> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(question)
        .execute(&mut *tx)
        .await?;
    let message_id = res.last_insert_rowid();
    sqlx::query(
        "INSERT INTO polls (message_id, question, allows_multi, anonymous, closes_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(question)
    .bind(allows_multi as i64)
    .bind(anonymous as i64)
    .bind(closes_at)
    .execute(&mut *tx)
    .await?;
    for (i, text) in options.iter().enumerate() {
        sqlx::query("INSERT INTO poll_options (message_id, position, text) VALUES (?, ?, ?)")
            .bind(message_id)
            .bind(i as i64)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(message_id)
}

/// LC-491: create an event (a poll with `event_at`/`event_location` set and a
/// fixed set of RSVP `options`). Single-choice, non-anonymous, never auto-closes
/// (RSVP stays open). Atomic, mirrors [`create`]. Returns the message id.
pub async fn create_event(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    title: &str,
    options: &[String],
    event_at: &str,
    event_location: Option<&str>,
) -> sqlx::Result<i64> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(title)
        .execute(&mut *tx)
        .await?;
    let message_id = res.last_insert_rowid();
    sqlx::query(
        "INSERT INTO polls \
             (message_id, question, allows_multi, anonymous, closes_at, event_at, event_location) \
         VALUES (?, ?, 0, 0, NULL, ?, ?)",
    )
    .bind(message_id)
    .bind(title)
    .bind(event_at)
    .bind(event_location)
    .execute(&mut *tx)
    .await?;
    for (i, text) in options.iter().enumerate() {
        sqlx::query("INSERT INTO poll_options (message_id, position, text) VALUES (?, ?, ?)")
            .bind(message_id)
            .bind(i as i64)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(message_id)
}

/// LC-491: events in a room - polls carrying an `event_at`, joined to their
/// (non-deleted) message. Drives the iCal feed. Returns
/// `(message_id, title, event_at, location)` ordered by start time.
pub async fn events_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> sqlx::Result<Vec<(i64, String, String, Option<String>)>> {
    let rows = sqlx::query(
        "SELECT p.message_id AS mid, p.question AS q, p.event_at AS ea, p.event_location AS loc \
         FROM polls p JOIN messages m ON m.id = p.message_id \
         WHERE m.room_id = ? AND m.deleted_at IS NULL AND p.event_at IS NOT NULL \
         ORDER BY p.event_at ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("mid"),
                r.get::<String, _>("q"),
                r.get::<String, _>("ea"),
                r.get::<Option<String>, _>("loc"),
            )
        })
        .collect())
}

fn map_poll(r: sqlx::sqlite::SqliteRow) -> Poll {
    Poll {
        message_id: r.get("message_id"),
        question: r.get("question"),
        allows_multi: r.get::<i64, _>("allows_multi") != 0,
        anonymous: r.get::<i64, _>("anonymous") != 0,
        closes_at: r.get("closes_at"),
        closed_at: r.get("closed_at"),
        event_at: r.get("event_at"),
        event_location: r.get("event_location"),
    }
}

/// LC-66: which of `message_ids` are polls. Lets the bulk message-history
/// loaders skip `build_poll_view` for ordinary messages with a single query
/// instead of one `polls` probe per message.
pub async fn poll_message_ids(
    pool: &SqlitePool,
    message_ids: &[i64],
) -> sqlx::Result<std::collections::HashSet<i64>> {
    if message_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT message_id FROM polls WHERE message_id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("message_id"))
        .collect())
}

/// LC-102: scheduled polls in a room - poll messages that carry a `closes_at`,
/// joined to their (non-deleted) message. Drives the iCal feed's VEVENT-per-
/// poll heuristic. Returns `(message_id, question, closes_at)` ordered by the
/// close time.
pub async fn scheduled_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> sqlx::Result<Vec<(i64, String, String)>> {
    let rows = sqlx::query(
        "SELECT p.message_id AS mid, p.question AS q, p.closes_at AS ca \
         FROM polls p JOIN messages m ON m.id = p.message_id \
         WHERE m.room_id = ? AND m.deleted_at IS NULL AND p.closes_at IS NOT NULL \
         ORDER BY p.closes_at ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("mid"),
                r.get::<String, _>("q"),
                r.get::<String, _>("ca"),
            )
        })
        .collect())
}

pub async fn get(pool: &SqlitePool, message_id: i64) -> sqlx::Result<Option<Poll>> {
    let row = sqlx::query(
        "SELECT message_id, question, allows_multi, anonymous, closes_at, closed_at, \
                event_at, event_location \
         FROM polls WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_poll))
}

/// Options for a poll, ordered by position, each with its vote count.
pub async fn options_with_counts(
    pool: &SqlitePool,
    message_id: i64,
) -> sqlx::Result<Vec<OptionTally>> {
    let rows = sqlx::query(
        "SELECT o.id, o.position, o.text, COUNT(v.user_id) AS n \
         FROM poll_options o \
         LEFT JOIN poll_votes v ON v.option_id = o.id \
         WHERE o.message_id = ? \
         GROUP BY o.id, o.position, o.text \
         ORDER BY o.position",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| OptionTally {
            id: r.get("id"),
            position: r.get("position"),
            text: r.get("text"),
            count: r.get("n"),
        })
        .collect())
}

/// Option ids the user has voted for in this poll.
pub async fn user_votes(
    pool: &SqlitePool,
    message_id: i64,
    user_id: &str,
) -> sqlx::Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT v.option_id FROM poll_votes v \
         JOIN poll_options o ON o.id = v.option_id \
         WHERE o.message_id = ? AND v.user_id = ?",
    )
    .bind(message_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("option_id"))
        .collect())
}

/// Distinct voter count across the whole poll (a multi-vote user counts
/// once). Used for the "N voted" caption.
pub async fn voter_count(pool: &SqlitePool, message_id: i64) -> sqlx::Result<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT v.user_id) FROM poll_votes v \
         JOIN poll_options o ON o.id = v.option_id \
         WHERE o.message_id = ?",
    )
    .bind(message_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Voter user-ids for one option (newest first). Callers must only invoke
/// this for non-anonymous polls; the anonymous flag is enforced one layer
/// up so voter identity never leaves the DB for an anonymous poll.
pub async fn voter_ids_for_option(pool: &SqlitePool, option_id: i64) -> sqlx::Result<Vec<String>> {
    let rows =
        sqlx::query("SELECT user_id FROM poll_votes WHERE option_id = ? ORDER BY voted_at DESC")
            .bind(option_id)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("user_id"))
        .collect())
}

/// True when `option_id` belongs to this poll. Guards votes against forged
/// option ids from other polls.
pub async fn option_belongs(
    pool: &SqlitePool,
    message_id: i64,
    option_id: i64,
) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT 1 FROM poll_options WHERE id = ? AND message_id = ?")
        .bind(option_id)
        .bind(message_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn add_vote(pool: &SqlitePool, option_id: i64, user_id: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO poll_votes (option_id, user_id) VALUES (?, ?)")
        .bind(option_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_vote(pool: &SqlitePool, option_id: i64, user_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM poll_votes WHERE option_id = ? AND user_id = ?")
        .bind(option_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove all of a user's votes in a poll (single-choice "move my vote").
pub async fn clear_user_votes(
    pool: &SqlitePool,
    message_id: i64,
    user_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM poll_votes WHERE user_id = ? AND option_id IN \
         (SELECT id FROM poll_options WHERE message_id = ?)",
    )
    .bind(user_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// A poll is closed once `closed_at` is set, or once `closes_at` has passed
/// (the latter covers the window between the deadline and the next closer
/// tick). Evaluated in SQL so the comparison matches the stored format.
pub async fn is_closed(pool: &SqlitePool, message_id: i64) -> sqlx::Result<bool> {
    let closed: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
           SELECT 1 FROM polls WHERE message_id = ? \
           AND (closed_at IS NOT NULL OR (closes_at IS NOT NULL AND closes_at <= datetime('now'))) \
         )",
    )
    .bind(message_id)
    .fetch_one(pool)
    .await?;
    Ok(closed)
}

/// Close every open poll whose deadline has passed; return `(message_id,
/// room_id)` for each newly closed poll so the caller can broadcast a
/// re-render. Used by the background closer tick.
pub async fn close_due(pool: &SqlitePool) -> sqlx::Result<Vec<(i64, i64)>> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        "SELECT p.message_id, m.room_id FROM polls p \
         JOIN messages m ON m.id = p.message_id \
         WHERE p.closed_at IS NULL AND p.closes_at IS NOT NULL AND p.closes_at <= datetime('now')",
    )
    .fetch_all(&mut *tx)
    .await?;
    let out: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (r.get::<i64, _>("message_id"), r.get::<i64, _>("room_id")))
        .collect();
    if !out.is_empty() {
        sqlx::query(
            "UPDATE polls SET closed_at = datetime('now') \
             WHERE closed_at IS NULL AND closes_at IS NOT NULL AND closes_at <= datetime('now')",
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(out)
}
