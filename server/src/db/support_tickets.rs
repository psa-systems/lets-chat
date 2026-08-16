//! LC-714: support tickets filed by the AI help desk (`/human`) when no admin
//! is available. Site admins triage the open queue at `/admin/support`. Modeled
//! on `db::reports` (LC-334): the same open -> resolved lifecycle, an
//! admin-topic live badge driven by `count_open`, and a `set_status` that only
//! transitions an open row so two admins cannot double-handle one.

use sqlx::{Row, SqlitePool};

use crate::models::support_ticket::SupportTicket;

/// Max characters kept from the user's request body (a row-growth bound; the
/// body is rendered escaped, never through the markdown pipeline).
pub const MAX_TICKET_BODY_CHARS: usize = 2000;

/// File a support ticket. Returns the new ticket id (so the requester can be
/// told "filed as #N").
pub async fn create(
    pool: &SqlitePool,
    requester_id: &str,
    room_id: Option<i64>,
    room_name: &str,
    body: &str,
) -> Result<i64, sqlx::Error> {
    let body: String = body.chars().take(MAX_TICKET_BODY_CHARS).collect();
    let result = sqlx::query(
        "INSERT INTO support_tickets (requester_id, room_id, room_name, body) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(requester_id)
    .bind(room_id)
    .bind(room_name)
    .bind(&body)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// LC-724: replace an open ticket's body with the richer context the requester
/// added from the support panel ("what you need / tried / urgency / contact").
/// Scoped to the owner and to the open state so a user can only enrich their own
/// still-open ticket (not one already claimed/resolved, and not someone else's).
/// Returns true when a row was updated.
pub async fn update_body(
    pool: &SqlitePool,
    id: i64,
    requester_id: &str,
    body: &str,
) -> Result<bool, sqlx::Error> {
    let body: String = body.chars().take(MAX_TICKET_BODY_CHARS).collect();
    let result = sqlx::query(
        "UPDATE support_tickets SET body = ? \
         WHERE id = ? AND requester_id = ? AND status = 'open'",
    )
    .bind(&body)
    .bind(id)
    .bind(requester_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Open tickets, newest first.
pub async fn list_open(pool: &SqlitePool) -> Result<Vec<SupportTicket>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, requester_id, room_id, room_name, body, status, handled_by, \
         created_at, handled_at \
         FROM support_tickets WHERE status = 'open' ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_ticket).collect())
}

/// One ticket by id (used to notify the requester on resolve).
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<SupportTicket>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, requester_id, room_id, room_name, body, status, handled_by, \
         created_at, handled_at \
         FROM support_tickets WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_ticket))
}

/// Count of open tickets (drives the nav badge).
pub async fn count_open(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM support_tickets WHERE status = 'open'")
        .fetch_one(pool)
        .await?;
    Ok(row.get("n"))
}

/// Resolve an open ticket. Only transitions a ticket still in the `open` state,
/// so two admins acting on the same row do not double-handle it. Returns `true`
/// when a row was updated.
pub async fn set_status(
    pool: &SqlitePool,
    id: i64,
    status: &str,
    handled_by: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE support_tickets \
         SET status = ?, handled_by = ?, handled_at = datetime('now') \
         WHERE id = ? AND status = 'open'",
    )
    .bind(status)
    .bind(handled_by)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

fn row_to_ticket(r: sqlx::sqlite::SqliteRow) -> SupportTicket {
    SupportTicket {
        id: r.get("id"),
        requester_id: r.get("requester_id"),
        room_id: r.get("room_id"),
        room_name: r.get("room_name"),
        body: r.get("body"),
        status: r.get("status"),
        handled_by: r.get("handled_by"),
        created_at: r.get("created_at"),
        handled_at: r.get("handled_at"),
    }
}
