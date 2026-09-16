//! LC-186: audit trail for remote-control sessions (LC-181).
//!
//! A session row is opened when the sharer grants control and closed when
//! control ends. Writes are driven from [`crate::routes::ws::relay_control_signal`]
//! (open on `grant`, close on `revoke`) with a socket-disconnect backstop that
//! closes any session a hard drop left open. The table stores only participant
//! ids + timestamps - no message content - so it is a traceability record, not
//! a content log.

use sqlx::{Row, SqlitePool};

/// LC-855: one row of the append-only remote-control consent audit
/// (`remote_control_events`). `kind` is request / grant / deny / revoke; the
/// ids are auth.db user ids resolved to names at render time.
pub struct RcEvent {
    pub room_id: i64,
    pub actor_id: String,
    pub target_id: String,
    pub kind: String,
    pub created_at: String,
}

/// LC-855: append one consent event to the audit. Best-effort at the call
/// sites (a logging failure must never block the relay), so this returns the
/// sqlx result for the caller to swallow. `kind` is a fixed vocabulary
/// (request/grant/deny/revoke); no validation here - the relays are the only
/// callers and pass constants.
pub async fn log_event(
    pool: &SqlitePool,
    room_id: i64,
    actor_id: &str,
    target_id: &str,
    kind: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO remote_control_events (room_id, actor_id, target_id, kind)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(room_id)
    .bind(actor_id)
    .bind(target_id)
    .bind(kind)
    .execute(pool)
    .await
    .map(|_| ())
}

/// LC-855: the most recent consent events, newest first, for the admin audit
/// listing. Bounded by `limit` so the page never renders an unbounded table.
pub async fn list_events(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<RcEvent>> {
    let rows = sqlx::query(
        "SELECT room_id, actor_id, target_id, kind, created_at
         FROM remote_control_events ORDER BY created_at DESC, id DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RcEvent {
            room_id: r.get("room_id"),
            actor_id: r.get("actor_id"),
            target_id: r.get("target_id"),
            kind: r.get("kind"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Open a session row for a freshly-granted control session. No-op if a row is
/// already open for the room, so a repeated `grant` (e.g. a re-grant after a
/// transient revoke) does not stack duplicate open rows.
pub async fn start_session(
    pool: &SqlitePool,
    room_id: i64,
    controller_id: &str,
    sharer_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO remote_control_sessions (room_id, controller_id, sharer_id)
         SELECT ?1, ?2, ?3
         WHERE NOT EXISTS (
             SELECT 1 FROM remote_control_sessions
             WHERE room_id = ?1 AND ended_at IS NULL
         )",
    )
    .bind(room_id)
    .bind(controller_id)
    .bind(sharer_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// LC-853: the open session for a room, if one exists. The huddle relay uses
/// this as the single-controller gate (a request while a row is open answers
/// `busy`) and to resolve who the counterpart of a `revoke` is.
pub struct OpenSession {
    pub controller_id: String,
    pub sharer_id: String,
}

/// LC-853: fetch the open (un-ended) session for `room_id`, if any.
pub async fn open_session(pool: &SqlitePool, room_id: i64) -> sqlx::Result<Option<OpenSession>> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT controller_id, sharer_id FROM remote_control_sessions
         WHERE room_id = ?1 AND ended_at IS NULL",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map(|(controller_id, sharer_id)| OpenSession {
            controller_id,
            sharer_id,
        })
    })
}

/// Close the open session for a room (kill-switch / revoke / auto-revoke).
/// Returns the row it actually closed, `None` if none was open - the RETURNING
/// clause makes the read and the write atomic, so a caller can notify from the
/// return value instead of a separate `open_session` read that could race a
/// concurrent closer.
pub async fn end_session_by_room(
    pool: &SqlitePool,
    room_id: i64,
    reason: &str,
) -> sqlx::Result<Option<OpenSession>> {
    let row = sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?2
         WHERE room_id = ?1 AND ended_at IS NULL
         RETURNING controller_id, sharer_id",
    )
    .bind(room_id)
    .bind(reason)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| OpenSession {
        controller_id: r.get("controller_id"),
        sharer_id: r.get("sharer_id"),
    }))
}

/// Close the room's open session only if `sharer_id` is the sharer of it (not
/// merely a party to it). Used by the share-stop auto-revoke, which must not
/// end a session where the stopping user was only the controller.
pub async fn end_session_by_sharer(
    pool: &SqlitePool,
    room_id: i64,
    sharer_id: &str,
    reason: &str,
) -> sqlx::Result<Option<OpenSession>> {
    let row = sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?3
         WHERE room_id = ?1 AND ended_at IS NULL AND sharer_id = ?2
         RETURNING controller_id, sharer_id",
    )
    .bind(room_id)
    .bind(sharer_id)
    .bind(reason)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| OpenSession {
        controller_id: r.get("controller_id"),
        sharer_id: r.get("sharer_id"),
    }))
}

/// Close the room's open session only if `user_id` is a party to it
/// (controller or sharer). The WHERE clause is the whole authorization *and*
/// the whole idempotency guard in one atomic statement: two concurrent callers
/// racing the same drop (e.g. the soft-leave handler and the hard-disconnect
/// backstop) can both run this, but `ended_at IS NULL` means only the first to
/// commit actually closes the row and gets `Some` back - the loser gets `None`
/// and must notify nobody.
pub async fn end_session_for_participant(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    reason: &str,
) -> sqlx::Result<Option<OpenSession>> {
    let row = sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?3
         WHERE room_id = ?1 AND ended_at IS NULL
         AND (controller_id = ?2 OR sharer_id = ?2)
         RETURNING controller_id, sharer_id",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(reason)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| OpenSession {
        controller_id: r.get("controller_id"),
        sharer_id: r.get("sharer_id"),
    }))
}

/// One session `end_sessions_for_user` actually closed, with the room it was
/// in (a user may be mid-session in more than one huddle at once).
pub struct ClosedSession {
    pub room_id: i64,
    pub controller_id: String,
    pub sharer_id: String,
}

/// Close every open session a user participates in (controller or sharer),
/// returning each one actually closed. The socket-disconnect backstop: a hard
/// WS drop never sends a `revoke`, so without this an interrupted session
/// would stay open forever. Same atomicity as `end_session_for_participant`:
/// the `ended_at IS NULL` guard means a session another writer already closed
/// (e.g. the soft-leave handler winning the same race) is not returned here.
pub async fn end_sessions_for_user(
    pool: &SqlitePool,
    user_id: &str,
    reason: &str,
) -> sqlx::Result<Vec<ClosedSession>> {
    let rows = sqlx::query(
        "UPDATE remote_control_sessions
         SET ended_at = datetime('now'), end_reason = ?2
         WHERE ended_at IS NULL AND (controller_id = ?1 OR sharer_id = ?1)
         RETURNING room_id, controller_id, sharer_id",
    )
    .bind(user_id)
    .bind(reason)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ClosedSession {
            room_id: r.get("room_id"),
            controller_id: r.get("controller_id"),
            sharer_id: r.get("sharer_id"),
        })
        .collect())
}
