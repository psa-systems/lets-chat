//! LC-91: APNs (iOS) device-token storage. Mirrors `push_subscriptions`
//! but keyed by the Apple-issued `device_token` instead of a Web Push
//! endpoint, and carries the APNs `topic` (app bundle id) the token was
//! registered against.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct ApnsSubscription {
    pub id: i64,
    pub user_id: String,
    pub device_token: String,
    pub topic: Option<String>,
    pub user_agent: Option<String>,
}

/// Insert a device token if unseen, else move it to `user_id` and refresh
/// the topic + user_agent. `device_token` is unique per install, so a second
/// user signing in on the same device inherits the row (same semantics as
/// the Web Push endpoint upsert).
pub async fn insert_or_replace(
    pool: &SqlitePool,
    user_id: &str,
    device_token: &str,
    topic: Option<&str>,
    user_agent: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "INSERT INTO apns_subscriptions \
             (user_id, device_token, topic, user_agent) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(device_token) DO UPDATE SET \
             user_id      = excluded.user_id, \
             topic        = excluded.topic, \
             user_agent   = excluded.user_agent, \
             last_seen_at = datetime('now') \
         RETURNING id",
    )
    .bind(user_id)
    .bind(device_token)
    .bind(topic)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;
    evict_over_cap(pool, user_id).await?;
    Ok(row.get("id"))
}

/// LC-147: keep at most `MAX_PUSH_SUBSCRIPTIONS_PER_USER` apns rows for
/// `user_id`, dropping the least-recently-seen first. See the Web Push
/// `evict_over_cap` for the tie-break rationale.
async fn evict_over_cap(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM apns_subscriptions \
          WHERE user_id = ?1 \
            AND id NOT IN ( \
                SELECT id FROM apns_subscriptions \
                 WHERE user_id = ?1 \
                 ORDER BY last_seen_at DESC, id DESC \
                 LIMIT ?2 \
            )",
    )
    .bind(user_id)
    .bind(crate::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<ApnsSubscription>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, device_token, topic, user_agent \
           FROM apns_subscriptions WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ApnsSubscription {
            id: r.get("id"),
            user_id: r.get("user_id"),
            device_token: r.get("device_token"),
            topic: r.get("topic"),
            user_agent: r.get("user_agent"),
        })
        .collect())
}

/// Delete a dead token (APNs replied `BadDeviceToken` / `Unregistered`).
pub async fn delete_by_token(pool: &SqlitePool, device_token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM apns_subscriptions WHERE device_token = ?")
        .bind(device_token)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn bump_last_seen(pool: &SqlitePool, device_token: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE apns_subscriptions SET last_seen_at = datetime('now') \
          WHERE device_token = ?",
    )
    .bind(device_token)
    .execute(pool)
    .await?;
    Ok(())
}
