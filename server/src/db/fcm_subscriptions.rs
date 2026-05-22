//! LC-91: FCM (Android) registration-token storage. Mirrors
//! `push_subscriptions` but keyed by the Firebase-issued
//! `registration_token` instead of a Web Push endpoint.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct FcmSubscription {
    pub id: i64,
    pub user_id: String,
    pub registration_token: String,
    pub user_agent: Option<String>,
}

/// Insert a registration token if unseen, else move it to `user_id` and
/// refresh the user_agent. The token is unique per install, so a second
/// user signing in on the same device inherits the row.
pub async fn insert_or_replace(
    pool: &SqlitePool,
    user_id: &str,
    registration_token: &str,
    user_agent: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "INSERT INTO fcm_subscriptions \
             (user_id, registration_token, user_agent) \
         VALUES (?, ?, ?) \
         ON CONFLICT(registration_token) DO UPDATE SET \
             user_id      = excluded.user_id, \
             user_agent   = excluded.user_agent, \
             last_seen_at = datetime('now') \
         RETURNING id",
    )
    .bind(user_id)
    .bind(registration_token)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

pub async fn for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<FcmSubscription>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, registration_token, user_agent \
           FROM fcm_subscriptions WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| FcmSubscription {
            id: r.get("id"),
            user_id: r.get("user_id"),
            registration_token: r.get("registration_token"),
            user_agent: r.get("user_agent"),
        })
        .collect())
}

/// Delete a dead token (FCM replied `NOT_REGISTERED` / `UNREGISTERED`).
pub async fn delete_by_token(
    pool: &SqlitePool,
    registration_token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM fcm_subscriptions WHERE registration_token = ?")
        .bind(registration_token)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn bump_last_seen(
    pool: &SqlitePool,
    registration_token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE fcm_subscriptions SET last_seen_at = datetime('now') \
          WHERE registration_token = ?",
    )
    .bind(registration_token)
    .execute(pool)
    .await?;
    Ok(())
}
