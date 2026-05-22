use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub id: i64,
    pub user_id: String,
    pub endpoint: String,
    pub p256dh_key: String,
    pub auth_key: String,
    pub user_agent: Option<String>,
}

/// Insert a subscription if its `endpoint` is unseen, else replace the
/// owning user (and refresh the keys + user_agent). Endpoint identifies
/// the (browser, application server) pair, so a second user logging in
/// on the same browser inherits the row.
pub async fn insert_or_replace(
    pool: &SqlitePool,
    user_id: &str,
    endpoint: &str,
    p256dh_key: &str,
    auth_key: &str,
    user_agent: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "INSERT INTO push_subscriptions \
             (user_id, endpoint, p256dh_key, auth_key, user_agent) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(endpoint) DO UPDATE SET \
             user_id      = excluded.user_id, \
             p256dh_key   = excluded.p256dh_key, \
             auth_key     = excluded.auth_key, \
             user_agent   = excluded.user_agent, \
             last_seen_at = datetime('now') \
         RETURNING id",
    )
    .bind(user_id)
    .bind(endpoint)
    .bind(p256dh_key)
    .bind(auth_key)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;
    evict_over_cap(pool, user_id).await?;
    Ok(row.get("id"))
}

/// LC-147: keep at most `MAX_PUSH_SUBSCRIPTIONS_PER_USER` rows for `user_id`,
/// dropping the least-recently-seen first. The row just upserted has
/// `last_seen_at = now`, so it always survives. Tie-break on `id` so a burst
/// of same-second registrations evicts deterministically (oldest id first).
async fn evict_over_cap(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM push_subscriptions \
          WHERE user_id = ?1 \
            AND id NOT IN ( \
                SELECT id FROM push_subscriptions \
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
) -> Result<Vec<PushSubscription>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, endpoint, p256dh_key, auth_key, user_agent \
           FROM push_subscriptions WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PushSubscription {
            id: r.get("id"),
            user_id: r.get("user_id"),
            endpoint: r.get("endpoint"),
            p256dh_key: r.get("p256dh_key"),
            auth_key: r.get("auth_key"),
            user_agent: r.get("user_agent"),
        })
        .collect())
}

pub async fn delete_by_endpoint(pool: &SqlitePool, endpoint: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = ?")
        .bind(endpoint)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn bump_last_seen(pool: &SqlitePool, endpoint: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE push_subscriptions SET last_seen_at = datetime('now') \
          WHERE endpoint = ?",
    )
    .bind(endpoint)
    .execute(pool)
    .await?;
    Ok(())
}
