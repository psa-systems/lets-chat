use sqlx::Row;

use crate::models::{Reaction, Room, SearchResult};

/// Two messages from the same author within this window are visually grouped:
/// the second is rendered as a "follow-up" (no username/timestamp header).
pub const MESSAGE_GROUPING_WINDOW_SECONDS: i64 = 300;

/// Pure predicate: would `(curr_user, curr_created_at)` render as a follow-up
/// of the immediately-prior message `(prev_user, prev_created_at)`?
///
/// Times are SQLite "YYYY-MM-DD HH:MM:SS" UTC strings. Returns `false` when
/// `prior` is `None` (first message in the thread).
pub fn is_follow_up_of(prior: Option<(&str, &str)>, curr: (&str, &str)) -> bool {
    let Some((prev_user, prev_at)) = prior else {
        return false;
    };
    if prev_user != curr.0 {
        return false;
    }
    let fmt = "%Y-%m-%d %H:%M:%S";
    let Ok(prev_dt) = chrono::NaiveDateTime::parse_from_str(prev_at, fmt) else {
        return false;
    };
    let Ok(curr_dt) = chrono::NaiveDateTime::parse_from_str(curr.1, fmt) else {
        return false;
    };
    let delta = curr_dt - prev_dt;
    delta >= chrono::Duration::zero()
        && delta <= chrono::Duration::seconds(MESSAGE_GROUPING_WINDOW_SECONDS)
}

/// Raw message row from the chat DB - contains user_id but no author_name.
/// The server fn layer resolves the display name from the auth DB.
#[derive(Debug, Clone)]
pub struct RawMessage {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// `Some(N)` when this message is a thread reply rooted at message `N`.
    /// `None` for top-level messages that appear in the main room timeline.
    pub parent_id: Option<i64>,
    /// `Some(N)` when this message is a quote-reply that visually quotes the
    /// message with id `N` inline above its body. Distinct from `parent_id`:
    /// quote-replies live in the main timeline rather than a side thread.
    pub quote_id: Option<i64>,
    /// True for server-authored system notices (e.g. "started a call").
    /// `user_id` still records who triggered the event; this only changes
    /// how the message renders.
    pub is_system: bool,
    /// LC-74: `Some(N)` for messages posted by incoming webhook `N`. Such
    /// rows carry `user_id = ''` and render with the webhook's name/avatar.
    pub webhook_id: Option<i64>,
    /// LC-77: `Some(N)` for messages posted by email-ingress inbox `N`.
    /// Parallel to `webhook_id`; same `user_id = ''` synthetic-actor shape.
    pub email_inbox_id: Option<i64>,
    /// LC-78: `Some(N)` for messages posted by protocol-bridge `N`. Unlike
    /// `webhook_id` and `email_inbox_id`, the actor identity is NOT
    /// resolved by joining to the bridge row at render time (the foreign
    /// actor set is open-ended). Instead, `bridge_foreign_name` and
    /// `bridge_kind` carry the per-message snapshot; this id is kept so
    /// the outgoing-webhook loop-break filter can identify "messages I
    /// myself produced." `None` for all non-bridge messages.
    pub bridge_id: Option<i64>,
    /// LC-78: snapshotted foreign display name for a bridge-posted message
    /// (e.g. Matrix `alice:server.org`). `Some` iff `bridge_id` is `Some`.
    /// Snapshotted (not joined) so the render survives bridge-row removal
    /// under stop-new lifecycle.
    pub bridge_foreign_name: Option<String>,
    /// LC-78: snapshotted protocol kind for a bridge-posted message
    /// (`matrix` / `irc` / `xmpp`). `Some` iff `bridge_id` is `Some`.
    pub bridge_kind: Option<String>,
    /// LC-78-AVATAR-PROXY: the cache key (sha256 of the canonical foreign
    /// avatar URL) snapshotted onto the row at POST time. `Some` when the
    /// daemon submitted `foreign_avatar` AND the proxy was enabled at
    /// submit time; otherwise `None` (render falls back to initials).
    /// Stored as a hash, not the URL, so the foreign URL never appears in
    /// rendered HTML (the structural side-channel closure that motivated
    /// schema-C in the design plan).
    pub bridge_foreign_avatar: Option<String>,
}

/// One archived prior version of a message. Inserted by `update_message_body`
/// before the live row is overwritten; surfaced to viewers by
/// `list_message_edits` via the `/messages/:id/history` endpoint.
#[derive(Debug, Clone)]
pub struct MessageEdit {
    pub id: i64,
    pub message_id: i64,
    pub previous_body: String,
    pub edited_at: String,
}

fn map_room(row: &sqlx::sqlite::SqliteRow) -> Room {
    Room {
        id: row.get("id"),
        name: row.get("name"),
        topic: row.get("topic"),
        room_type: row.get("room_type"),
        invite_code: row.get("invite_code"),
        created_at: row.get("created_at"),
        is_voice: row.get("is_voice"),
        posting_allowed_for: row.get("posting_allowed_for"),
        description: row.get("description"),
        wiki_body: row.get("wiki_body"),
        wiki_updated_at: row.get("wiki_updated_at"),
        wiki_updated_by: row.get("wiki_updated_by"),
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
            "SELECT id, name, topic, room_type, invite_code, created_at, is_voice, posting_allowed_for, description, wiki_body, wiki_updated_at, wiki_updated_by \
             FROM rooms WHERE room_type != 'dm' ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        return Ok(rows.iter().map(map_room).collect());
    }

    let rows = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, r.is_voice, r.posting_allowed_for, r.description, r.wiki_body, r.wiki_updated_at, r.wiki_updated_by \
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
        "SELECT id, name, topic, room_type, invite_code, created_at, is_voice, posting_allowed_for, description, wiki_body, wiki_updated_at, wiki_updated_by FROM rooms WHERE id = ?",
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
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE room_id = ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
         ORDER BY id ASC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_raw).collect())
}

/// LC-78: cursor-paginated read of top-level messages in a room. Forked
/// from `list_messages` (which the web/HTMX room render still uses,
/// unbounded + ascending) so the API can return bounded pages without
/// touching the page-render's shape.
///
/// `before_id`: optional cursor; results are strictly older than this id.
/// `limit`: capped to `MAX_PAGINATED_LIMIT` by the caller. Returned in
/// `id DESC` order so the API caller can walk backwards through history
/// by feeding the oldest returned `id` back as `before_id` on the next
/// request. Same visibility filter as `list_messages`.
pub async fn list_messages_paginated(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    // Two branches because sqlx's `bind` is positional; threading an
    // `Option<i64>` through the same query string would still need the
    // placeholder, and `WHERE ? IS NULL OR id < ?` defeats the index.
    let rows = if let Some(cursor) = before_id {
        sqlx::query(
            "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
             FROM messages \
             WHERE room_id = ? AND id < ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
             ORDER BY id DESC LIMIT ?",
        )
        .bind(room_id)
        .bind(cursor)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
             FROM messages \
             WHERE room_id = ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
             ORDER BY id DESC LIMIT ?",
        )
        .bind(room_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    Ok(rows.into_iter().map(row_to_raw).collect())
}

fn row_to_raw(row: sqlx::sqlite::SqliteRow) -> RawMessage {
    RawMessage {
        id: row.get("id"),
        room_id: row.get("room_id"),
        user_id: row.get("user_id"),
        body: row.get("body"),
        created_at: row.get("created_at"),
        edited_at: row.get("edited_at"),
        parent_id: row.get("parent_id"),
        quote_id: row.get("quote_id"),
        is_system: row.get("is_system"),
        webhook_id: row.get("webhook_id"),
        email_inbox_id: row.get("email_inbox_id"),
        bridge_id: row.get("bridge_id"),
        bridge_foreign_name: row.get("bridge_foreign_name"),
        bridge_kind: row.get("bridge_kind"),
        bridge_foreign_avatar: row.get("bridge_foreign_avatar"),
    }
}

/// LC-102: the most recent top-level messages in a room, newest first, capped
/// at `limit`. Same visibility filter as `list_messages` (no deleted /
/// quarantined / thread replies). Used to build the room's RSS/Atom feed.
pub async fn list_recent_messages(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    limit: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE room_id = ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
         ORDER BY id DESC LIMIT ?",
    )
    .bind(room_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_raw).collect())
}

/// Replies in a thread, ordered chronologically. Excludes soft-deleted rows.
/// Caller must verify access to the parent's room before calling.
pub async fn list_thread_replies(
    pool: &sqlx::SqlitePool,
    parent_id: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE parent_id = ? AND deleted_at IS NULL AND quarantined = 0 \
         ORDER BY id ASC",
    )
    .bind(parent_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_raw).collect())
}

/// Reply count per top-level message in a room, returned as `(parent_id,
/// reply_count)`. Used to render the "N replies" pill under each message.
pub async fn count_replies_for_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT parent_id AS pid, COUNT(*) AS c \
         FROM messages \
         WHERE room_id = ? AND parent_id IS NOT NULL AND deleted_at IS NULL AND quarantined = 0 \
         GROUP BY parent_id",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<i64, _>("pid"), r.get::<i64, _>("c")))
        .collect())
}

pub async fn count_replies(pool: &sqlx::SqlitePool, parent_id: i64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS c FROM messages \
         WHERE parent_id = ? AND deleted_at IS NULL AND quarantined = 0",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("c"))
}

pub async fn insert_reply(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
    parent_id: i64,
) -> Result<i64, sqlx::Error> {
    let result =
        sqlx::query("INSERT INTO messages (room_id, user_id, body, parent_id) VALUES (?, ?, ?, ?)")
            .bind(room_id)
            .bind(user_id)
            .bind(body)
            .bind(parent_id)
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

/// Fetch the most recent non-deleted message in `room_id` strictly before
/// `before_id` (by id). Returns `None` if `before_id` is the first message in
/// the room. Used to compute `is_follow_up` for a message rendered in
/// isolation (POST handler and WS new-message broadcast).
pub async fn prior_message_in_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    before_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE room_id = ? AND id < ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(room_id)
    .bind(before_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_raw))
}

/// Fetch the next non-deleted message in `room_id` strictly after `after_id`
/// (by id). Returns `None` if `after_id` is the last message in the room.
/// Used by the delete handler to repair grouping when a header is removed.
pub async fn next_message_in_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    after_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE room_id = ? AND id > ? AND deleted_at IS NULL AND quarantined = 0 AND parent_id IS NULL \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(room_id)
    .bind(after_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_raw))
}

/// Fetch a single message by ID. Returns None if soft-deleted.
pub async fn get_message(
    pool: &sqlx::SqlitePool,
    message_id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages WHERE id = ? AND deleted_at IS NULL AND quarantined = 0",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_raw))
}

/// Update a message's body and set edited_at to now. Returns the edited_at timestamp.
///
/// Wraps the prior-body archive (INSERT into message_edits) and the live-row
/// UPDATE in a single transaction so the history rows and messages.body never
/// disagree about which version was displaced. The SELECT runs inside the tx
/// rather than reusing a body the caller fetched earlier, so the archived
/// previous_body matches what the UPDATE actually overwrites even if a
/// concurrent edit commits in between.
pub async fn update_message_body(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    new_body: &str,
) -> Result<String, sqlx::Error> {
    let edited_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut tx = pool.begin().await?;
    let prior_body: String = sqlx::query_scalar("SELECT body FROM messages WHERE id = ?")
        .bind(message_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO message_edits (message_id, previous_body, edited_at) VALUES (?, ?, ?)",
    )
    .bind(message_id)
    .bind(&prior_body)
    .bind(&edited_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE messages SET body = ?, edited_at = ? WHERE id = ?")
        .bind(new_body)
        .bind(&edited_at)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(edited_at)
}

/// List archived prior versions of a message, oldest-first. Empty for
/// unedited messages. The live (current) body lives in `messages.body` and
/// is appended by the handler after this read.
pub async fn list_message_edits(
    pool: &sqlx::SqlitePool,
    message_id: i64,
) -> Result<Vec<MessageEdit>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, message_id, previous_body, edited_at \
         FROM message_edits WHERE message_id = ? \
         ORDER BY edited_at ASC, id ASC",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| MessageEdit {
            id: row.get("id"),
            message_id: row.get("message_id"),
            previous_body: row.get("previous_body"),
            edited_at: row.get("edited_at"),
        })
        .collect())
}

pub async fn insert_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    insert_message_quoted(pool, room_id, user_id, body, None).await
}

/// Insert a server-authored system message (e.g. "started a call"). The
/// `user_id` still records who triggered the event; `is_system = 1` switches
/// the rendering to a centered, non-interactive notice.
pub async fn insert_system_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result =
        sqlx::query("INSERT INTO messages (room_id, user_id, body, is_system) VALUES (?, ?, ?, 1)")
            .bind(room_id)
            .bind(user_id)
            .bind(body)
            .execute(pool)
            .await?;
    Ok(result.last_insert_rowid())
}

/// Like [`insert_message`] but additionally records a `quote_id` reference
/// to the message being quoted. Pass `None` for a plain top-level message.
/// LC-74: insert a message authored by an incoming webhook. Stores an empty
/// user_id plus the webhook id; rendering resolves the name/avatar from the
/// webhook row.
pub async fn insert_webhook_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    webhook_id: i64,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO messages (room_id, user_id, webhook_id, body) VALUES (?, '', ?, ?)",
    )
    .bind(room_id)
    .bind(webhook_id)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// LC-77: insert a message authored by an email-ingress inbox. Mirrors
/// `insert_webhook_message`: empty user_id, the inbox id stored in
/// `email_inbox_id` (NULL `webhook_id`); rendering resolves name/avatar
/// from the `email_inboxes` row.
pub async fn insert_email_inbox_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    email_inbox_id: i64,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO messages (room_id, user_id, email_inbox_id, body) VALUES (?, '', ?, ?)",
    )
    .bind(room_id)
    .bind(email_inbox_id)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// LC-78: insert a message authored by a protocol bridge. Unlike
/// `insert_webhook_message` / `insert_email_inbox_message`, the foreign
/// actor identity (display name + protocol kind) is SNAPSHOTTED onto
/// the row at post time, because the set of foreign actors is open-ended
/// and there is no per-channel row to join back to at render time. The
/// bridge daemon's role / room access has already been verified by the
/// caller. `foreign_avatar` is reserved nullable for the LC-78-AVATAR-PROXY
/// follow-up; v1 always passes `None` (the endpoint 400s any non-null
/// foreign avatar URL).
pub async fn insert_bridge_message(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    bridge_id: i64,
    foreign_name: &str,
    foreign_avatar: Option<&str>,
    kind: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO messages (room_id, user_id, bridge_id, bridge_foreign_name, bridge_foreign_avatar, bridge_kind, body) \
         VALUES (?, '', ?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(bridge_id)
    .bind(foreign_name)
    .bind(foreign_avatar)
    .bind(kind)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn insert_message_quoted(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
    quote_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result =
        sqlx::query("INSERT INTO messages (room_id, user_id, body, quote_id) VALUES (?, ?, ?, ?)")
            .bind(room_id)
            .bind(user_id)
            .bind(body)
            .bind(quote_id)
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
    enclave_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO rooms (name, topic, room_type, invite_code, enclave_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(topic)
    .bind(room_type)
    .bind(invite_code)
    .bind(enclave_id)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Like [`create_room`] but flags the room as a voice channel. Visibility
/// (`room_type` = "public" | "private") still applies and is orthogonal to
/// the voice flag.
pub async fn create_voice_room(
    pool: &sqlx::SqlitePool,
    name: &str,
    topic: Option<&str>,
    room_type: &str,
    invite_code: Option<&str>,
    enclave_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO rooms (name, topic, room_type, invite_code, enclave_id, is_voice) \
         VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(name)
    .bind(topic)
    .bind(room_type)
    .bind(invite_code)
    .bind(enclave_id)
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

/// LC-86: set the long-form description (markdown source). Pass
/// `None` (or an empty string filtered out by the caller) to clear.
pub async fn set_room_description(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    description: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET description = ? WHERE id = ?")
        .bind(description)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-86: write the wiki body and stamp last-edit metadata. `None` body
/// clears the wiki (sets wiki_body / wiki_updated_at / wiki_updated_by
/// all NULL). Returns the number of rows updated.
pub async fn set_room_wiki(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    wiki_body: Option<&str>,
    actor_user_id: &str,
) -> Result<u64, sqlx::Error> {
    let res = if wiki_body.is_some() {
        sqlx::query(
            "UPDATE rooms SET wiki_body = ?, wiki_updated_at = datetime('now'), \
             wiki_updated_by = ? WHERE id = ?",
        )
        .bind(wiki_body)
        .bind(actor_user_id)
        .bind(room_id)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            "UPDATE rooms SET wiki_body = NULL, wiki_updated_at = NULL, \
             wiki_updated_by = NULL WHERE id = ?",
        )
        .bind(room_id)
        .execute(pool)
        .await?
    };
    Ok(res.rows_affected())
}

/// LC-85: set the per-room "who can post" policy. Caller is expected to
/// pre-validate the policy string against
/// `("all", "moderators_only", "admins_only")`; the CHECK constraint
/// in the schema is the second line of defence. Returns the number of
/// rows updated (0 if `room_id` does not exist).
pub async fn set_room_posting_policy(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    policy: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET posting_allowed_for = ? WHERE id = ?")
        .bind(policy)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Read the per-room retention policy. `None` means retention is
/// disabled; `Some(N)` means messages older than N days are eligible
/// for the retention sweep. The CHECK in migration 0043 enforces
/// `N IS NULL OR N >= 1` at write time.
pub async fn get_room_retention_days(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT retention_days FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await
        .map(|opt| opt.flatten())
}

/// Set (or clear) the per-room retention policy. Pass `None` to disable
/// retention on the room; `Some(N >= 1)` to enable. Returns the number
/// of rows updated (0 if `room_id` does not exist). The route handler
/// validates the `N >= 1` floor before calling this; the schema CHECK
/// is the second line of defence.
pub async fn set_room_retention_days(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    days: Option<i64>,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET retention_days = ? WHERE id = ?")
        .bind(days)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn list_rooms_in_enclave(
    pool: &sqlx::SqlitePool,
    enclave_id: i64,
    user_id: &str,
    can_see_all_private: bool,
) -> Result<Vec<Room>, sqlx::Error> {
    if can_see_all_private {
        let rows = sqlx::query(
            "SELECT id, name, topic, room_type, invite_code, created_at, is_voice, posting_allowed_for, description, wiki_body, wiki_updated_at, wiki_updated_by \
             FROM rooms WHERE enclave_id=? AND room_type != 'dm' ORDER BY name",
        )
        .bind(enclave_id)
        .fetch_all(pool)
        .await?;
        return Ok(rows.iter().map(map_room).collect());
    }
    let rows = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, r.is_voice, r.posting_allowed_for, r.description, r.wiki_body, r.wiki_updated_at, r.wiki_updated_by \
         FROM rooms r \
         LEFT JOIN room_members m ON m.room_id = r.id AND m.user_id = ? \
         WHERE r.enclave_id=? AND r.room_type != 'dm' \
           AND (r.room_type='public' OR m.user_id IS NOT NULL) \
         ORDER BY r.name",
    )
    .bind(user_id)
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_room).collect())
}

/// Predicate combining DM, public-in-enclave, and private-room rules.
/// `is_site_admin` short-circuits to true.
pub async fn is_room_accessible(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    is_site_admin: bool,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT room_type, enclave_id FROM rooms WHERE id=?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    let Some(r) = row else {
        return Ok(false);
    };
    let room_type: String = r.get("room_type");
    let enclave_id: Option<i64> = r.get("enclave_id");

    // Site admin god-mode applies to existing rooms only.
    if is_site_admin && room_type != "dm" {
        return Ok(true);
    }
    if is_site_admin && room_type == "dm" {
        return is_room_member(pool, room_id, user_id).await;
    }

    if room_type == "dm" {
        return is_room_member(pool, room_id, user_id).await;
    }

    let Some(eid) = enclave_id else {
        return Ok(false);
    };
    let in_enclave = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(eid)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .is_some();
    if !in_enclave {
        return Ok(false);
    }

    // Public channels (text or voice) are open to every enclave member;
    // private channels still require explicit membership. The voice flag is
    // orthogonal and does not affect access.
    if room_type == "public" {
        return Ok(true);
    }
    is_room_member(pool, room_id, user_id).await
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

/// List the user_ids of all members of a room.
pub async fn list_room_member_ids(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id FROM room_members WHERE room_id = ?")
        .bind(room_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.get("user_id")).collect())
}

/// Find a room by its invite code.
pub async fn get_room_by_invite(
    pool: &sqlx::SqlitePool,
    invite_code: &str,
) -> Result<Option<Room>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, topic, room_type, invite_code, created_at, is_voice, posting_allowed_for, description, wiki_body, wiki_updated_at, wiki_updated_by \
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
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, r.is_voice, r.posting_allowed_for, r.description, r.wiki_body, r.wiki_updated_at, r.wiki_updated_by \
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
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, r.is_voice, r.posting_allowed_for, r.description, r.wiki_body, r.wiki_updated_at, r.wiki_updated_by, m2.user_id as other_user \
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
          AND m.deleted_at IS NULL AND m.quarantined = 0 \
          AND m.parent_id IS NULL \
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
          AND m.deleted_at IS NULL AND m.quarantined = 0 \
          AND m.parent_id IS NULL \
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
          AND m.deleted_at IS NULL AND m.quarantined = 0 \
          AND m.parent_id IS NULL \
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

/// Count messages in `room_id` newer than `user_id`'s last-read watermark
/// authored by anyone other than `user_id` and not soft-deleted. Used to
/// re-render a single sidebar unread badge after a live event.
pub async fn get_unread_count(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(m.id) AS unread \
         FROM messages m \
         LEFT JOIN dm_read_state s ON s.room_id = m.room_id AND s.user_id = ? \
         WHERE m.room_id = ? \
           AND m.user_id != ? \
           AND m.deleted_at IS NULL AND m.quarantined = 0 \
           AND m.parent_id IS NULL \
           AND m.id > COALESCE(s.last_read_message_id, 0)",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("unread"))
}

/// Find the highest own-authored message id in a DM that the peer has read,
/// along with the peer's read timestamp. Used to render the "Seen" caption
/// under the most recent own message that the peer has acknowledged. Returns
/// None when the peer has not read any of `viewer_id`'s messages yet.
pub async fn find_dm_seen_state(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    viewer_id: &str,
    peer_id: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT MAX(m.id) AS msg_id, s.updated_at AS read_at \
         FROM dm_read_state s \
         JOIN messages m \
           ON m.room_id = s.room_id \
          AND m.user_id = ? \
          AND m.deleted_at IS NULL AND m.quarantined = 0 \
          AND m.id <= s.last_read_message_id \
         WHERE s.room_id = ? AND s.user_id = ?",
    )
    .bind(viewer_id)
    .bind(room_id)
    .bind(peer_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| {
        let id: Option<i64> = r.get("msg_id");
        id.map(|i| (i, r.get::<String, _>("read_at")))
    }))
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

/// LC-250: the highest message id in a room, or `None` for an empty room.
/// Used by "mark all as read" to advance the viewer's read watermark to the
/// latest message in each conversation that has unread.
pub async fn latest_message_id(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(id) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await
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
         WHERE m.room_id = ? AND m.deleted_at IS NULL AND m.quarantined = 0 \
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
/// Scope rules:
/// - `enclave_id_filter = Some(eid)`: only non-DM rooms where r.enclave_id = eid.
///   The caller must be a member of the enclave; the route layer enforces this
///   before invoking the function. Private rooms still require room_members
///   unless `is_site_admin = true`.
/// - `enclave_id_filter = None` and `home_scope = true`: search every room
///   the caller can read - DM rooms they are in, plus non-DM rooms in any
///   enclave they belong to (with private rooms still gated on room_members).
///   Site admins additionally see every non-DM room regardless of enclave
///   membership; their DM scope still requires explicit room_members so they
///   never silently page through other people's DMs.
/// - `enclave_id_filter = None` and `home_scope = false`: rejected upstream;
///   this combination is not produced by the route handler.
///
/// `room_id_filter` further narrows the result to a single room when set.
pub async fn search_messages(
    pool: &sqlx::SqlitePool,
    fts_query: &str,
    room_id_filter: Option<i64>,
    enclave_id_filter: Option<i64>,
    home_scope: bool,
    caller_user_id: &str,
    is_site_admin: bool,
) -> Result<Vec<SearchResult>, sqlx::Error> {
    let room_filter_clause = match room_id_filter {
        Some(_) => "AND m.room_id = ?",
        None => "",
    };

    let scope_clause = if enclave_id_filter.is_some() {
        // Inside an enclave: rooms in that enclave only, and either site admin
        // or caller must be in private-room members for private rooms.
        if is_site_admin {
            "AND r.enclave_id = ? AND r.room_type != 'dm'"
        } else {
            "AND r.enclave_id = ? AND r.room_type != 'dm' \
             AND (r.room_type = 'public' OR EXISTS (\
                 SELECT 1 FROM room_members rm \
                 WHERE rm.room_id = r.id AND rm.user_id = ?))"
        }
    } else if home_scope {
        if is_site_admin {
            "AND ( \
                (r.room_type = 'dm' AND EXISTS (\
                    SELECT 1 FROM room_members rm \
                    WHERE rm.room_id = r.id AND rm.user_id = ?)) \
                OR r.room_type != 'dm' \
            )"
        } else {
            "AND ( \
                (r.room_type = 'dm' AND EXISTS (\
                    SELECT 1 FROM room_members rm \
                    WHERE rm.room_id = r.id AND rm.user_id = ?)) \
                OR ( \
                    r.room_type != 'dm' \
                    AND r.enclave_id IS NOT NULL \
                    AND EXISTS (\
                        SELECT 1 FROM enclave_members em \
                        WHERE em.enclave_id = r.enclave_id AND em.user_id = ?) \
                    AND (r.room_type = 'public' OR EXISTS (\
                        SELECT 1 FROM room_members rm2 \
                        WHERE rm2.room_id = r.id AND rm2.user_id = ?)) \
                ) \
            )"
        }
    } else {
        // No usable scope; return nothing rather than leak global results.
        "AND 0 = 1"
    };

    let sql = format!(
        "SELECT m.id AS message_id, m.room_id, r.name AS room_name, \
                m.body, m.user_id, m.created_at \
         FROM messages_fts \
         JOIN messages m ON m.id = messages_fts.rowid \
         JOIN rooms    r ON r.id  = m.room_id \
         WHERE messages_fts MATCH ? \
           AND m.deleted_at IS NULL AND m.quarantined = 0 \
           {room_filter_clause} \
           {scope_clause} \
         ORDER BY messages_fts.rank \
         LIMIT 50"
    );

    let mut q = sqlx::query(&sql).bind(fts_query);
    if let Some(rid) = room_id_filter {
        q = q.bind(rid);
    }
    if let Some(eid) = enclave_id_filter {
        q = q.bind(eid);
        if !is_site_admin {
            q = q.bind(caller_user_id);
        }
    } else if home_scope {
        if is_site_admin {
            q = q.bind(caller_user_id);
        } else {
            q = q
                .bind(caller_user_id)
                .bind(caller_user_id)
                .bind(caller_user_id);
        }
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
            author_name: r.get::<String, _>("user_id"),
            created_at: r.get("created_at"),
        })
        .collect())
}
