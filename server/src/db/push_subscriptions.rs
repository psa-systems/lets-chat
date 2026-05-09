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
    Ok(row.get("id"))
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
