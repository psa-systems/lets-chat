use sqlx::Row;

use crate::models::{Reaction, Room, SearchResult};

/// Two messages from the same author within this window are visually grouped:
/// the second is rendered as a "follow-up" (no username/timestamp header).
///
/// LC-387 widened this to 15 min; LC-435 narrows it to 5 min. 15 min bundled
/// messages sent far apart under one header, so a single visual group stretched
/// across a real time gap and the spacing read as random. At 5 min a rapid
/// burst still groups tightly, but a same-author message sent minutes/hours
/// later breaks into a fresh block with its own header + avatar + timestamp -
/// giving time-separated messages a clear anchor. The follow-up still surfaces
/// its own HH:MM on row hover (LC-377), and a UTC day change still forces a
/// fresh header (the same-day check below). Shared by the room + DM render.
pub const MESSAGE_GROUPING_WINDOW_SECONDS: i64 = 300;

/// Pure predicate: would `(curr_user, curr_created_at)` render as a follow-up
/// of the immediately-prior message `(prev_user, prev_created_at)`?
///
/// Times are SQLite "YYYY-MM-DD HH:MM:SS" UTC strings. Returns `false` when
/// `prior` is `None` (first message in the thread). A grouped run breaks on a
/// different author, a gap over the window, OR a UTC day change (LC-387) - the
/// last so a near-midnight follow-up never renders headerless under the day
/// divider the client inserts.
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
    // LC-387: a day change always starts a fresh header, even within the window.
    if prev_dt.date() != curr_dt.date() {
        return false;
    }
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

/// List channels visible to a user (DMs are never included).
///
/// LC-606: this used to be `room_type = 'public' OR member`, with no enclave
/// condition, so it returned channels from enclaves the caller is not in - the
/// same drift LC-604 fixed for the unread queries. It feeds the forward-message
/// destination picker and the rooms API, so those listed rooms the caller would
/// be refused on open. Visibility now comes from [`accessible_rooms_sql`], the
/// shared predicate that mirrors [`is_room_accessible`].
///
/// Admins still see every channel: the admin arm of the fragment reduces to
/// `room_type != 'dm'` here, which is what the separate admin query did.
pub async fn list_rooms(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<Room>, sqlx::Error> {
    let sql = format!(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at, r.is_voice, r.posting_allowed_for, r.description, r.wiki_body, r.wiki_updated_at, r.wiki_updated_by \
         FROM rooms r \
         WHERE r.room_type != 'dm' \
           AND {access} \
         ORDER BY r.name",
        access = accessible_rooms_sql(is_admin),
    );

    let mut q = sqlx::query(&sql);
    for _ in 0..accessible_rooms_binds(is_admin) {
        q = q.bind(user_id);
    }
    let rows = q.fetch_all(pool).await?;

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

/// LC-679: the enclave a room belongs to, or `None` for a DM / enclave-less
/// room. Used by the AI feature gate to resolve enclave Owner/Admin scope.
pub async fn room_enclave_id(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT enclave_id FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<i64>, _>("enclave_id")))
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

/// LC-797: replies the thread panel renders per page. Matches the panel's
/// visible height plus a scroll buffer; older pages arrive via the sentinel.
pub const THREAD_REPLY_PAGE_LIMIT: i64 = 50;

/// Replies in a thread, ordered chronologically. Excludes soft-deleted rows.
/// Caller must verify access to the parent's room before calling.
///
/// LC-797: UNBOUNDED, and no longer on any render path. The only callers left
/// are the two LLM digest paths (`routes::summary::summarize_thread` and
/// `routes::thread_title`), which must read the whole thread to summarize it.
/// Renders go through `list_thread_replies_page`.
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

/// LC-797: cursor-paginated read of a thread's replies, the thread-panel
/// counterpart of `list_messages_paginated`. Same visibility filter as
/// `list_thread_replies`.
///
/// `before_id`: optional cursor; results are strictly older than this id.
/// Returned in `id DESC` order so the caller walks backwards through the thread
/// by feeding the oldest returned `id` back as the next `before_id`.
pub async fn list_thread_replies_paginated(
    pool: &sqlx::SqlitePool,
    parent_id: i64,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<RawMessage>, sqlx::Error> {
    // Two branches for the same reason as `list_messages_paginated`: sqlx binds
    // positionally, and `WHERE ? IS NULL OR id < ?` defeats the index.
    let rows = if let Some(cursor) = before_id {
        sqlx::query(
            "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
             FROM messages \
             WHERE parent_id = ? AND id < ? AND deleted_at IS NULL AND quarantined = 0 \
             ORDER BY id DESC LIMIT ?",
        )
        .bind(parent_id)
        .bind(cursor)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
             FROM messages \
             WHERE parent_id = ? AND deleted_at IS NULL AND quarantined = 0 \
             ORDER BY id DESC LIMIT ?",
        )
        .bind(parent_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    Ok(rows.into_iter().map(row_to_raw).collect())
}

/// LC-797: one render page of a thread's replies, returned oldest-first
/// alongside whether still-older replies exist behind the page.
///
/// Reads `limit + 1` rows and reports the overflow as `has_older`, so the panel
/// can decide whether to emit a load-older sentinel without a second COUNT.
pub async fn list_thread_replies_page(
    pool: &sqlx::SqlitePool,
    parent_id: i64,
    before_id: Option<i64>,
    limit: i64,
) -> Result<(Vec<RawMessage>, bool), sqlx::Error> {
    let mut rows = list_thread_replies_paginated(pool, parent_id, before_id, limit + 1).await?;
    let has_older = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    rows.reverse();
    Ok((rows, has_older))
}

/// LC-806: the `(user_id, created_at)` of the visible reply immediately older
/// than `before_id` in a thread, i.e. the predecessor a live-appended reply
/// groups against (`is_follow_up_of`). `None` when it is the thread's first
/// reply. Same visibility filter as the page reads.
pub async fn thread_reply_prior(
    pool: &sqlx::SqlitePool,
    parent_id: i64,
    before_id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT user_id, created_at FROM messages \
         WHERE parent_id = ? AND id < ? AND deleted_at IS NULL AND quarantined = 0 \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(parent_id)
    .bind(before_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| (r.get("user_id"), r.get("created_at"))))
}

/// LC-806: one visible reply of a thread by id, as the same `RawMessage` the
/// page reads produce, so the load-older fragment can re-render the on-screen
/// boundary row against its new predecessor. `None` when `id` is not a visible
/// reply of `parent_id`.
pub async fn thread_reply_raw(
    pool: &sqlx::SqlitePool,
    parent_id: i64,
    id: i64,
) -> Result<Option<RawMessage>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, room_id, user_id, body, created_at, edited_at, parent_id, quote_id, is_system, webhook_id, email_inbox_id, bridge_id, bridge_foreign_name, bridge_kind, bridge_foreign_avatar \
         FROM messages \
         WHERE parent_id = ? AND id = ? AND deleted_at IS NULL AND quarantined = 0",
    )
    .bind(parent_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_raw))
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

/// LC-668: the AI-generated title for a thread (stored on its root message), or
/// None if it has not been generated yet.
pub async fn get_thread_title(
    pool: &sqlx::SqlitePool,
    message_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT thread_title FROM messages WHERE id = ?")
            .bind(message_id)
            .fetch_optional(pool)
            .await?;
    Ok(v.flatten())
}

/// LC-668: store a generated thread title on the root message.
pub async fn set_thread_title(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    title: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET thread_title = ? WHERE id = ?")
        .bind(title)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
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
/// LC-676: replace a message body in place WITHOUT recording an edit or setting
/// `edited_at`. Used to swap the assistant's "thinking..." placeholder for its
/// answer, so the answer does not render as "(edited)". The FTS `UPDATE OF body`
/// trigger still keeps search in sync.
pub async fn replace_message_body(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    new_body: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET body = ? WHERE id = ?")
        .bind(new_body)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

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

/// LC-547: stamp a message's self-destruct time. `expires_at` is a
/// `"%Y-%m-%d %H:%M:%S"` UTC string (see [`crate::models::message::ephemeral_expires_at`]);
/// the unconditional ephemeral sweep hard-deletes the row once that time is in
/// the past. Called by `post_message` only when the sender attached a TTL, so a
/// message without a timer keeps its default NULL (permanent).
pub async fn set_message_expiry(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    expires_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET expires_at = ? WHERE id = ?")
        .bind(expires_at)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
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

/// LC-534: the room's slowmode interval in seconds (0 = off).
pub async fn get_room_slowmode(pool: &sqlx::SqlitePool, room_id: i64) -> Result<u32, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar("SELECT slowmode_seconds FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(v.unwrap_or(0).max(0) as u32)
}

/// LC-534: set the room's slowmode interval in seconds. Returns rows updated.
pub async fn set_room_slowmode(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    seconds: u32,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET slowmode_seconds = ? WHERE id = ?")
        .bind(seconds as i64)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-476: the room's `@here`/`@channel` broadcast policy. Read on demand
/// (not carried on the `Room` struct) - mirrors `get_room_retention_days`.
/// A missing row yields `'all'` (the permissive default).
/// LC-492: whether the in-channel AI assistant (`/ask`) is enabled for a room.
pub async fn get_room_assistant_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar("SELECT assistant_enabled FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(v.unwrap_or(0) != 0)
}

/// LC-492: toggle the assistant for a room. Returns rows affected (0 = no such
/// room).
pub async fn set_room_assistant_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    enabled: bool,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET assistant_enabled = ? WHERE id = ?")
        .bind(enabled as i64)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-665: whether the scheduled AI activity digest is enabled for a room.
pub async fn get_room_digest_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar("SELECT digest_enabled FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(v.unwrap_or(0) != 0)
}

/// LC-665: toggle the scheduled digest for a room. Returns rows affected.
pub async fn set_room_digest_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    enabled: bool,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET digest_enabled = ? WHERE id = ?")
        .bind(enabled as i64)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-665: the last time a digest ran for a room (ISO-8601 UTC), or None if it
/// never has. Read before bumping to window the messages "since last digest".
pub async fn get_room_digest_last_at(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT digest_last_at FROM rooms WHERE id = ?")
            .bind(room_id)
            .fetch_optional(pool)
            .await?;
    Ok(v.flatten())
}

/// LC-665: rooms whose digest is enabled and due - never run, or last run more
/// than `interval_hours` ago. The cutoff is computed in SQL against `now`.
pub async fn rooms_due_for_digest(
    pool: &sqlx::SqlitePool,
    interval_hours: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let modifier = format!("-{interval_hours} hours");
    sqlx::query_scalar(
        "SELECT id FROM rooms WHERE digest_enabled = 1 \
         AND (digest_last_at IS NULL OR digest_last_at < datetime('now', ?))",
    )
    .bind(modifier)
    .fetch_all(pool)
    .await
}

/// LC-665: mark a room's digest as just run (dedupe marker). Bumped on every
/// evaluation, whether or not a digest was actually posted, so a quiet room is
/// not re-evaluated until the next interval.
pub async fn set_room_digest_last_at(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE rooms SET digest_last_at = datetime('now') WHERE id = ?")
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-494: whether "stage" mode (large-audience audio) is enabled for a room.
pub async fn get_room_stage_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar("SELECT stage_enabled FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(pool)
        .await?;
    Ok(v.unwrap_or(0) != 0)
}

/// LC-494: toggle stage mode for a room. Returns rows affected (0 = no such
/// room).
pub async fn set_room_stage_enabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    enabled: bool,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET stage_enabled = ? WHERE id = ?")
        .bind(enabled as i64)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-855: whether this room has opted OUT of remote control. Layered under the
/// workspace `settings.remote_control_enabled` master switch: control is offered
/// only when the workspace switch is on AND this is false.
pub async fn get_room_remote_control_disabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT remote_control_disabled FROM rooms WHERE id = ?")
            .bind(room_id)
            .fetch_optional(pool)
            .await?;
    Ok(v.unwrap_or(0) != 0)
}

/// LC-855: set the per-room remote-control opt-out. Returns rows affected
/// (0 = no such room).
pub async fn set_room_remote_control_disabled(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    disabled: bool,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET remote_control_disabled = ? WHERE id = ?")
        .bind(disabled as i64)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// LC-492: lightweight room-scoped FTS retrieval for the AI assistant. Returns
/// up to `limit` `(author_user_id, body)` pairs from the room ranked by FTS
/// relevance to `fts_query` (already sanitized via `sanitize_fts_query`).
/// Deleted / quarantined / system messages are excluded. Unlike
/// `search_messages_filtered` this skips access scoping: the caller (the `/ask`
/// dispatcher) has already passed the room access + posting gates.
pub async fn fts_room_context(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT m.user_id, m.body \
           FROM messages_fts \
           JOIN messages m ON m.id = messages_fts.rowid \
          WHERE messages_fts MATCH ? AND m.room_id = ? \
            AND m.deleted_at IS NULL AND m.quarantined = 0 AND m.is_system = 0 \
          ORDER BY messages_fts.rank \
          LIMIT ?",
    )
    .bind(fts_query)
    .bind(room_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("user_id"), r.get("body")))
        .collect())
}

pub async fn get_room_broadcast_policy(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<String, sqlx::Error> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT broadcast_allowed_for FROM rooms WHERE id = ?")
            .bind(room_id)
            .fetch_optional(pool)
            .await?;
    Ok(v.unwrap_or_else(|| "all".to_string()))
}

/// LC-476: set the room's broadcast policy. Returns rows affected (0 = no such
/// room). Mirrors `set_room_posting_policy`.
pub async fn set_room_broadcast_policy(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    policy: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE rooms SET broadcast_allowed_for = ? WHERE id = ?")
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

/// LC-489: user_ids of room members (excluding `viewer_id`) whose read
/// watermark has reached the room's latest non-deleted message - i.e. who have
/// "seen" everything. Returns ids only; the caller resolves consent
/// (`read_receipts_enabled`) and display labels from auth.db. Empty when the
/// room has no messages (`MAX(id)` is NULL, so the `>=` never holds).
pub async fn room_caught_up_member_ids(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    viewer_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT s.user_id \
           FROM dm_read_state s \
           JOIN room_members rm ON rm.room_id = s.room_id AND rm.user_id = s.user_id \
          WHERE s.room_id = ? AND s.user_id != ? \
            AND s.last_read_message_id >= ( \
                SELECT MAX(id) FROM messages \
                 WHERE room_id = ? AND deleted_at IS NULL AND quarantined = 0)",
    )
    .bind(room_id)
    .bind(viewer_id)
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.get("user_id")).collect())
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

/// LC-637: the user_ids of every member of the enclave that owns `room_id`.
/// A public room is reachable only by members of its enclave (see
/// `is_room_accessible`), so its live events fan out to exactly this set - not
/// every connected socket, which both leaked message bodies to non-members and
/// cost a server-wide send per message. Returns empty for a room with no
/// enclave (e.g. a dm), which never takes the public fan-out arm.
pub async fn list_enclave_member_ids_for_room(
    pool: &sqlx::SqlitePool,
    room_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT em.user_id FROM enclave_members em \
         JOIN rooms r ON r.enclave_id = em.enclave_id \
         WHERE r.id = ?",
    )
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

/// LC-286: set the read watermark to an EXACT value, even a lower one. Unlike
/// `upsert_dm_read` (which keeps `MAX` so the read paths only advance), this
/// overwrites unconditionally so "mark unread" can move the watermark backward.
pub async fn rewind_dm_read(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    room_id: i64,
    last_read_message_id: i64,
) -> Result<(), sqlx::Error> {
    let updated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO dm_read_state (user_id, room_id, last_read_message_id, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, room_id) DO UPDATE SET \
           last_read_message_id = excluded.last_read_message_id, \
           updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(last_read_message_id)
    .bind(&updated_at)
    .execute(pool)
    .await?;
    Ok(())
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

/// LC-604: the single source of truth for "which rooms may this viewer read",
/// as a SQL fragment, so list queries cannot drift from [`is_room_accessible`]
/// the way `list_unread` and `list_room_unread_counts` did. Both had encoded
/// `room_type = 'public' OR member`, which reads "public" as public to the whole
/// instance - but a room is only public *within its enclave*, and
/// `is_room_accessible` (which backs `require_room_access`, and so the 403s)
/// requires enclave membership first. A non-member was therefore refused a room
/// while still seeing its name, unread count, and newest message body.
///
/// Mirrors [`is_room_accessible`] branch for branch: site admins get every
/// non-DM room and their own DMs; everyone else needs enclave membership for a
/// channel (plus room membership when it is private) and room membership for a
/// DM. A channel with no `enclave_id` is unreachable for non-admins, as there.
///
/// The fragment assumes the `rooms` table is aliased `r`. Every placeholder
/// takes the viewer's user id - bind it [`accessible_rooms_binds`] times, in
/// order, wherever the fragment is spliced in.
pub fn accessible_rooms_sql(is_admin: bool) -> &'static str {
    if is_admin {
        "(r.room_type != 'dm' \
          OR EXISTS (SELECT 1 FROM room_members rm \
                      WHERE rm.room_id = r.id AND rm.user_id = ?))"
    } else {
        "((r.room_type = 'dm' \
           AND EXISTS (SELECT 1 FROM room_members rm \
                        WHERE rm.room_id = r.id AND rm.user_id = ?)) \
          OR (r.room_type != 'dm' \
              AND r.enclave_id IS NOT NULL \
              AND EXISTS (SELECT 1 FROM enclave_members em \
                           WHERE em.enclave_id = r.enclave_id AND em.user_id = ?) \
              AND (r.room_type = 'public' \
                   OR EXISTS (SELECT 1 FROM room_members rm \
                               WHERE rm.room_id = r.id AND rm.user_id = ?))))"
    }
}

/// How many times to bind the viewer's user id for [`accessible_rooms_sql`].
pub fn accessible_rooms_binds(is_admin: bool) -> usize {
    if is_admin {
        1
    } else {
        3
    }
}

/// For each non-DM room visible to the user, count messages from other users
/// that are newer than the caller's watermark in dm_read_state. The
/// dm_read_state table is room-keyed, so we reuse it for non-DM rooms as well.
///
/// Visibility comes from [`accessible_rooms_sql`], so this matches what
/// `require_room_access` will actually allow (LC-604).
pub async fn list_room_unread_counts(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let sql = format!(
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
           AND {access} \
         GROUP BY r.id",
        access = accessible_rooms_sql(is_admin),
    );

    // Bind order follows placeholder order: dm_read_state, messages author,
    // then the access fragment's own placeholders.
    let mut q = sqlx::query(&sql).bind(user_id).bind(user_id);
    for _ in 0..accessible_rooms_binds(is_admin) {
        q = q.bind(user_id);
    }

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
/// Returns `true` if the reaction is present after the call, `false` if removed.
///
/// LC-553: this used to be `SELECT` then a separate `INSERT`/`DELETE` with no
/// transaction, so two concurrent toggles from the same user (a double-tap)
/// could both observe "absent" and both `INSERT`; the second violated the
/// `(message_id, user_id, emoji)` primary key and surfaced as a 500, leaving the
/// state "reacted" even though the pair of taps meant on-then-off. Each branch
/// is now a single atomic statement keyed on rows-affected, and the insert is
/// `OR IGNORE`, so a lost race is a no-op rather than an error and the returned
/// flag always matches the row's final presence.
pub async fn toggle_reaction(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    user_id: &str,
    emoji: &str,
) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query(
        "DELETE FROM message_reactions WHERE message_id = ? AND user_id = ? AND emoji = ?",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?
    .rows_affected();

    if deleted > 0 {
        return Ok(false);
    }

    // Not present (or a concurrent toggle removed it first): add it. `OR IGNORE`
    // makes a concurrent insert a no-op instead of a primary-key error; either
    // way the row is present when this returns, so report `true`.
    sqlx::query(
        "INSERT OR IGNORE INTO message_reactions (message_id, user_id, emoji) VALUES (?, ?, ?)",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?;
    Ok(true)
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
                MAX(CASE WHEN user_id = ? THEN 1 ELSE 0 END) AS reacted_by_me, \
                group_concat(user_id) AS reactor_ids \
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
            reactor_ids: split_reactor_ids(r.get::<Option<String>, _>("reactor_ids")),
        })
        .collect())
}

/// Parse the `group_concat(user_id)` blob from a reaction aggregate into a
/// `Vec<String>`. SQLite joins with a literal comma and user ids never contain
/// commas, so a plain split is safe. NULL / empty -> empty vec.
fn split_reactor_ids(raw: Option<String>) -> Vec<String> {
    match raw {
        Some(s) if !s.is_empty() => s.split(',').map(|p| p.to_string()).collect(),
        _ => Vec::new(),
    }
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
                MAX(CASE WHEN mr.user_id = ? THEN 1 ELSE 0 END) AS reacted_by_me, \
                group_concat(mr.user_id) AS reactor_ids \
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
                    reactor_ids: split_reactor_ids(r.get::<Option<String>, _>("reactor_ids")),
                },
            )
        })
        .collect())
}

/// LC-477: the caller's most-used Unicode reaction emoji, most-frequent first.
/// Powers the one-tap quick-react bar so a user's actual habits (cross-device,
/// frequency-ranked) seed the bar instead of only device-local MRU. Custom
/// `:shortcode:` reactions are excluded - they are enclave-scoped and the
/// quick-react bar/recents path deliberately skips them (LC-288/LC-302). Ties
/// break toward the more recently used glyph. `limit` caps the row count.
pub async fn top_reaction_emojis(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT emoji \
         FROM message_reactions \
         WHERE user_id = ? AND emoji NOT LIKE ':%' \
         GROUP BY emoji \
         ORDER BY COUNT(*) DESC, MAX(created_at) DESC \
         LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| r.get("emoji")).collect())
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

/// LC-676: FTS query for the /ask RAG retrieval. Unlike [`sanitize_fts_query`]
/// (which joins terms with a space = FTS5 implicit AND, right for a deliberate
/// search box), a natural-language question - "who is david?" - should retrieve
/// messages matching ANY meaningful term. ANDing every word means the question
/// only matches a message that literally contains "who" AND "is" AND "david",
/// so a room clearly discussing David still dead-ended. This drops common
/// question stopwords and joins the rest with OR; `fts_room_context` then ranks
/// the candidates. Returns `None` only when nothing usable remains.
pub fn fts_query_any(raw: &str) -> Option<String> {
    const STOP: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "am", "who", "whom", "whose",
        "what", "when", "where", "why", "how", "which", "that", "this", "these", "those", "to",
        "of", "in", "on", "at", "for", "from", "with", "and", "or", "do", "does", "did", "can",
        "could", "would", "should", "will", "about", "tell", "me", "us", "please", "i", "you",
        "it", "he", "she", "they", "we", "any", "know", "there",
    ];
    // Trim FTS-special and punctuation from each word's edges ("david?" ->
    // "david", "(david)" -> "david") while keeping internal characters.
    let clean: Vec<String> = raw
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect();
    if clean.is_empty() {
        return None;
    }
    // Prefer the content words; if the question was ALL stopwords, keep them so
    // the query still retrieves something.
    let mut kept: Vec<&String> = clean
        .iter()
        .filter(|t| !STOP.contains(&t.to_ascii_lowercase().as_str()))
        .collect();
    if kept.is_empty() {
        kept = clean.iter().collect();
    }
    Some(
        kept.iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
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
/// LC-280: optional refinements parsed from search operators (`from:`,
/// `before:`, `after:`). All `None` = a plain search (the `search_messages`
/// shim passes this). `before`/`after` are `YYYY-MM-DD` strings compared
/// lexicographically against `created_at` ("YYYY-MM-DD HH:MM:SS"), which is
/// correct for this fixed ISO-ish format.
#[derive(Default, Clone, Debug)]
pub struct SearchFilters {
    /// Restrict to messages authored by this user id.
    pub author_id: Option<String>,
    /// Only messages strictly before this date (`created_at < before`).
    pub before: Option<String>,
    /// Only messages on/after this date (`created_at >= after`).
    pub after: Option<String>,
    /// LC-530: only messages with at least one file attachment (`has:file`).
    pub has_file: bool,
    /// LC-530: only messages whose body contains an http(s) URL (`has:link`).
    pub has_link: bool,
    /// LC-530: only messages that are a thread reply (`in:thread`).
    pub in_thread: bool,
}

/// Plain full-text search (no operator refinements). Thin shim over
/// [`search_messages_filtered`] so existing callers stay unchanged.
pub async fn search_messages(
    pool: &sqlx::SqlitePool,
    fts_query: &str,
    room_id_filter: Option<i64>,
    enclave_id_filter: Option<i64>,
    home_scope: bool,
    caller_user_id: &str,
    is_site_admin: bool,
) -> Result<Vec<SearchResult>, sqlx::Error> {
    search_messages_filtered(
        pool,
        fts_query,
        room_id_filter,
        enclave_id_filter,
        home_scope,
        caller_user_id,
        is_site_admin,
        &SearchFilters::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn search_messages_filtered(
    pool: &sqlx::SqlitePool,
    fts_query: &str,
    room_id_filter: Option<i64>,
    enclave_id_filter: Option<i64>,
    home_scope: bool,
    caller_user_id: &str,
    is_site_admin: bool,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, sqlx::Error> {
    let room_filter_clause = match room_id_filter {
        Some(_) => "AND m.room_id = ?",
        None => "",
    };

    // LC-280: operator refinements. Clauses are appended AFTER scope_clause and
    // their binds AFTER the scope binds, in author/before/after order, to keep
    // the existing positional bind order intact.
    let author_clause = if filters.author_id.is_some() {
        "AND m.user_id = ?"
    } else {
        ""
    };
    let before_clause = if filters.before.is_some() {
        "AND m.created_at < ?"
    } else {
        ""
    };
    let after_clause = if filters.after.is_some() {
        "AND m.created_at >= ?"
    } else {
        ""
    };
    // LC-530: bind-free boolean refinements (has:file / has:link / in:thread).
    // No `?`, so they do not affect the positional bind order below.
    let has_file_clause = if filters.has_file {
        "AND EXISTS (SELECT 1 FROM file_uploads fu WHERE fu.message_id = m.id)"
    } else {
        ""
    };
    let has_link_clause = if filters.has_link {
        "AND (m.body LIKE '%http://%' OR m.body LIKE '%https://%')"
    } else {
        ""
    };
    let in_thread_clause = if filters.in_thread {
        "AND m.parent_id IS NOT NULL"
    } else {
        ""
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
           {author_clause} \
           {before_clause} \
           {after_clause} \
           {has_file_clause} \
           {has_link_clause} \
           {in_thread_clause} \
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
    // LC-280: operator binds, last and in clause order (author, before, after).
    if let Some(author_id) = &filters.author_id {
        q = q.bind(author_id);
    }
    if let Some(before) = &filters.before {
        q = q.bind(before);
    }
    if let Some(after) = &filters.after {
        q = q.bind(after);
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

/// LC-549: fetch `SearchResult` rows for a specific set of message ids within one
/// room, returned in the given id order. Used by semantic search after cosine
/// ranking has chosen the ids. Room access is enforced by the caller (the
/// semantic path already gates on `is_room_accessible`); deleted / quarantined
/// rows are filtered so a stale embedding never surfaces a removed message.
pub async fn search_results_for_ids(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    ids: &[i64],
) -> Result<Vec<SearchResult>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT m.id AS message_id, m.room_id, r.name AS room_name, \
                m.body, m.user_id, m.created_at \
         FROM messages m JOIN rooms r ON r.id = m.room_id \
         WHERE m.room_id = ? AND m.deleted_at IS NULL AND m.quarantined = 0 \
           AND m.id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql).bind(room_id);
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let mut by_id: std::collections::HashMap<i64, SearchResult> =
        std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let mid: i64 = r.get("message_id");
        by_id.insert(
            mid,
            SearchResult {
                message_id: mid,
                room_id: r.get("room_id"),
                room_name: r.get("room_name"),
                body: r.get("body"),
                user_id: r.get("user_id"),
                author_name: r.get::<String, _>("user_id"),
                created_at: r.get("created_at"),
            },
        );
    }
    // Preserve the caller's rank order.
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}
