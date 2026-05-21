//! LC-75: outgoing webhooks + delivery bookkeeping.

use sqlx::{Row, SqlitePool};

/// Full webhook row for the admin page. `signing_secret` is included so the
/// page can reveal it (it is the receiver's verification key).
#[derive(Debug, Clone)]
pub struct OutgoingWebhook {
    pub id: i64,
    pub scope_kind: String,
    pub scope_id: Option<i64>,
    pub events: String,
    pub url: String,
    pub signing_secret: String,
    pub created_at: String,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub consecutive_failures: i64,
    pub disabled_at: Option<String>,
}

/// Minimal fields the dispatcher needs to deliver + sign.
#[derive(Debug, Clone)]
pub struct WebhookTarget {
    pub id: i64,
    pub url: String,
    pub signing_secret: String,
    pub disabled_at: Option<String>,
}

/// A pending delivery the dispatcher should attempt.
#[derive(Debug, Clone)]
pub struct DueDelivery {
    pub id: i64,
    pub webhook_id: i64,
    pub event: String,
    pub payload: String,
    pub attempt: i64,
}

/// One delivery row for the per-webhook history view.
#[derive(Debug, Clone)]
pub struct DeliveryRow {
    pub id: i64,
    pub event: String,
    pub attempt: i64,
    pub status: Option<i64>,
    pub scheduled_at: String,
    pub delivered_at: Option<String>,
}

fn map_webhook(r: sqlx::sqlite::SqliteRow) -> OutgoingWebhook {
    OutgoingWebhook {
        id: r.get("id"),
        scope_kind: r.get("scope_kind"),
        scope_id: r.get("scope_id"),
        events: r.get("events"),
        url: r.get("url"),
        signing_secret: r.get("signing_secret"),
        created_at: r.get("created_at"),
        last_success_at: r.get("last_success_at"),
        last_failure_at: r.get("last_failure_at"),
        consecutive_failures: r.get("consecutive_failures"),
        disabled_at: r.get("disabled_at"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &SqlitePool,
    scope_kind: &str,
    scope_id: Option<i64>,
    events: &str,
    url: &str,
    signing_secret: &str,
    created_by: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO outgoing_webhooks (scope_kind, scope_id, events, url, signing_secret, created_by) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(events)
    .bind(url)
    .bind(signing_secret)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn list_all(pool: &SqlitePool) -> sqlx::Result<Vec<OutgoingWebhook>> {
    let rows = sqlx::query(
        "SELECT id, scope_kind, scope_id, events, url, signing_secret, created_at, \
                last_success_at, last_failure_at, consecutive_failures, disabled_at \
         FROM outgoing_webhooks ORDER BY id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_webhook).collect())
}

pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<OutgoingWebhook>> {
    let row = sqlx::query(
        "SELECT id, scope_kind, scope_id, events, url, signing_secret, created_at, \
                last_success_at, last_failure_at, consecutive_failures, disabled_at \
         FROM outgoing_webhooks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_webhook))
}

/// Enabled webhooks subscribed to `event` whose scope matches a message in
/// `room_id` (with enclave `enclave_id`): global, the room's enclave, or the
/// room itself. Event match is a whitespace-token check on the `events` list.
pub async fn matching(
    pool: &SqlitePool,
    event: &str,
    room_id: i64,
    enclave_id: Option<i64>,
) -> sqlx::Result<Vec<i64>> {
    let rows = sqlx::query(
        "SELECT id, events FROM outgoing_webhooks \
         WHERE disabled_at IS NULL AND ( \
             scope_kind = 'global' \
             OR (scope_kind = 'room' AND scope_id = ?) \
             OR (scope_kind = 'enclave' AND scope_id = ?) \
         )",
    )
    .bind(room_id)
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| {
            r.get::<String, _>("events")
                .split_whitespace()
                .any(|e| e == event)
        })
        .map(|r| r.get::<i64, _>("id"))
        .collect())
}

pub async fn enqueue_delivery(
    pool: &SqlitePool,
    webhook_id: i64,
    event: &str,
    payload: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO outgoing_webhook_deliveries (webhook_id, event, payload) VALUES (?, ?, ?)",
    )
    .bind(webhook_id)
    .bind(event)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Pending deliveries that are due (`scheduled_at <= now`), oldest first.
pub async fn due_deliveries(pool: &SqlitePool, limit: i64) -> sqlx::Result<Vec<DueDelivery>> {
    let rows = sqlx::query(
        "SELECT id, webhook_id, event, payload, attempt \
         FROM outgoing_webhook_deliveries \
         WHERE delivered_at IS NULL AND scheduled_at <= datetime('now') \
         ORDER BY scheduled_at ASC, id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DueDelivery {
            id: r.get("id"),
            webhook_id: r.get("webhook_id"),
            event: r.get("event"),
            payload: r.get("payload"),
            attempt: r.get("attempt"),
        })
        .collect())
}

pub async fn target(pool: &SqlitePool, webhook_id: i64) -> sqlx::Result<Option<WebhookTarget>> {
    let row = sqlx::query(
        "SELECT id, url, signing_secret, disabled_at FROM outgoing_webhooks WHERE id = ?",
    )
    .bind(webhook_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| WebhookTarget {
        id: r.get("id"),
        url: r.get("url"),
        signing_secret: r.get("signing_secret"),
        disabled_at: r.get("disabled_at"),
    }))
}

/// Mark a delivery succeeded (stores HTTP status).
pub async fn mark_delivered(pool: &SqlitePool, id: i64, status: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outgoing_webhook_deliveries \
         SET delivered_at = datetime('now'), status = ?, attempt = attempt + 1 WHERE id = ?",
    )
    .bind(status)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Reschedule a delivery for a future retry. `status` is the HTTP code or 0
/// for a network error; `delay_secs` is the backoff.
pub async fn reschedule(
    pool: &SqlitePool,
    id: i64,
    status: i64,
    response_body: Option<&str>,
    delay_secs: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outgoing_webhook_deliveries \
         SET attempt = attempt + 1, status = ?, response_body = ?, \
             scheduled_at = datetime('now', ? || ' seconds') WHERE id = ?",
    )
    .bind(status)
    .bind(response_body)
    .bind(delay_secs)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a delivery permanently failed (retries exhausted).
pub async fn mark_failed(
    pool: &SqlitePool,
    id: i64,
    status: i64,
    response_body: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outgoing_webhook_deliveries \
         SET delivered_at = datetime('now'), status = ?, response_body = ?, \
             attempt = attempt + 1 WHERE id = ?",
    )
    .bind(status)
    .bind(response_body)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_success(pool: &SqlitePool, webhook_id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outgoing_webhooks \
         SET last_success_at = datetime('now'), consecutive_failures = 0 WHERE id = ?",
    )
    .bind(webhook_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed delivery (retries exhausted). Increments the consecutive
/// counter and auto-disables once it reaches `disable_threshold`.
pub async fn record_failure(
    pool: &SqlitePool,
    webhook_id: i64,
    disable_threshold: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outgoing_webhooks \
         SET last_failure_at = datetime('now'), \
             consecutive_failures = consecutive_failures + 1, \
             disabled_at = CASE \
                 WHEN consecutive_failures + 1 >= ? AND disabled_at IS NULL \
                 THEN datetime('now') ELSE disabled_at END \
         WHERE id = ?",
    )
    .bind(disable_threshold)
    .bind(webhook_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Rotate the signing secret.
pub async fn rotate_secret(pool: &SqlitePool, id: i64, new_secret: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE outgoing_webhooks SET signing_secret = ? WHERE id = ?")
        .bind(new_secret)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Enable or disable a webhook. Enabling clears the failure counter so a
/// re-enabled hook starts fresh.
pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> sqlx::Result<()> {
    if enabled {
        sqlx::query(
            "UPDATE outgoing_webhooks SET disabled_at = NULL, consecutive_failures = 0 WHERE id = ?",
        )
        .bind(id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE outgoing_webhooks SET disabled_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM outgoing_webhooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Recent delivery attempts for a webhook (newest first).
pub async fn deliveries_for(
    pool: &SqlitePool,
    webhook_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<DeliveryRow>> {
    let rows = sqlx::query(
        "SELECT id, event, attempt, status, scheduled_at, delivered_at \
         FROM outgoing_webhook_deliveries WHERE webhook_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(webhook_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DeliveryRow {
            id: r.get("id"),
            event: r.get("event"),
            attempt: r.get("attempt"),
            status: r.get("status"),
            scheduled_at: r.get("scheduled_at"),
            delivered_at: r.get("delivered_at"),
        })
        .collect())
}

/// Retention: keep only the newest `keep` delivery rows per webhook.
pub async fn prune_deliveries(pool: &SqlitePool, keep: i64) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM outgoing_webhook_deliveries WHERE id IN ( \
            SELECT id FROM ( \
                SELECT id, ROW_NUMBER() OVER (PARTITION BY webhook_id ORDER BY id DESC) AS rn \
                FROM outgoing_webhook_deliveries \
            ) WHERE rn > ? \
         )",
    )
    .bind(keep)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
