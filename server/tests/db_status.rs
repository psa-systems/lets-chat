use lets_chat::db::auth as auth_db;
use sqlx::SqlitePool;

mod common;

async fn setup_pool() -> SqlitePool {
    common::auth_pool().await
}

#[tokio::test]
async fn defaults_to_active_with_no_custom() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "alice", "hash").await.unwrap();
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "active");
    assert!(u.custom_status.is_none());
    assert!(!u.last_active_at.is_empty());
}

#[tokio::test]
async fn set_user_status_persists_known_values_and_custom_text() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "bob", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "dnd", Some("debugging"), None)
        .await
        .unwrap();
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "dnd");
    assert_eq!(u.custom_status.as_deref(), Some("debugging"));
}

#[tokio::test]
async fn set_user_status_rejects_invalid_status() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "bob", "hash").await.unwrap();
    let err = auth_db::set_user_status(&pool, &id, "offline", None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, auth_db::SetStatusError::InvalidStatus));
}

#[tokio::test]
async fn set_user_status_rejects_overlong_custom() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "bob", "hash").await.unwrap();
    let too_long = "x".repeat(51);
    let err = auth_db::set_user_status(&pool, &id, "active", Some(&too_long), None)
        .await
        .unwrap_err();
    assert!(matches!(err, auth_db::SetStatusError::CustomTooLong(50)));
}

#[tokio::test]
async fn touch_user_activity_flips_idle_back_to_active() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "carol", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "idle", None, None)
        .await
        .unwrap();
    let flipped = auth_db::touch_user_activity(&pool, &id).await.unwrap();
    assert!(flipped, "expected idle->active flip to be reported");
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "active");
}

#[tokio::test]
async fn touch_user_activity_leaves_dnd_alone() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "dave", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "dnd", Some("focus"), None)
        .await
        .unwrap();
    let flipped = auth_db::touch_user_activity(&pool, &id).await.unwrap();
    assert!(!flipped);
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "dnd");
    assert_eq!(u.custom_status.as_deref(), Some("focus"));
}

#[tokio::test]
async fn touch_user_activity_no_flip_when_already_active() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "eve", "hash").await.unwrap();
    let flipped = auth_db::touch_user_activity(&pool, &id).await.unwrap();
    assert!(!flipped);
}

#[tokio::test]
async fn mark_idle_users_only_flips_stale_active_rows() {
    let pool = setup_pool().await;
    let stale = auth_db::create_user(&pool, "stale", "hash").await.unwrap();
    let fresh = auth_db::create_user(&pool, "fresh", "hash").await.unwrap();
    let dnd = auth_db::create_user(&pool, "dnd", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &dnd, "dnd", None, None)
        .await
        .unwrap();

    sqlx::query("UPDATE users SET last_active_at = datetime('now', '-1 hour') WHERE id IN (?, ?)")
        .bind(&stale)
        .bind(&dnd)
        .execute(&pool)
        .await
        .unwrap();

    let flipped = auth_db::mark_idle_users(&pool, 30 * 60).await.unwrap();
    assert_eq!(flipped, vec![stale.clone()]);

    let stale_u = auth_db::find_user_by_id(&pool, &stale)
        .await
        .unwrap()
        .unwrap();
    let fresh_u = auth_db::find_user_by_id(&pool, &fresh)
        .await
        .unwrap()
        .unwrap();
    let dnd_u = auth_db::find_user_by_id(&pool, &dnd)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale_u.status, "idle");
    assert_eq!(fresh_u.status, "active");
    assert_eq!(dnd_u.status, "dnd");
}

#[tokio::test]
async fn is_user_dnd_reads_current_status() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "frank", "hash").await.unwrap();
    assert!(!auth_db::is_user_dnd(&pool, &id).await.unwrap());
    auth_db::set_user_status(&pool, &id, "dnd", None, None)
        .await
        .unwrap();
    assert!(auth_db::is_user_dnd(&pool, &id).await.unwrap());
}

#[tokio::test]
async fn set_user_status_accepts_away() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "grace", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "away", Some("lunch"), None)
        .await
        .unwrap();
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "away");
    assert_eq!(u.custom_status.as_deref(), Some("lunch"));
}

#[tokio::test]
async fn touch_user_activity_leaves_away_alone() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "heidi", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "away", None, None)
        .await
        .unwrap();
    let flipped = auth_db::touch_user_activity(&pool, &id).await.unwrap();
    assert!(!flipped, "manual away must not flip back to active");
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.status, "away");
}

#[tokio::test]
async fn mark_idle_users_leaves_away_alone() {
    let pool = setup_pool().await;
    let away = auth_db::create_user(&pool, "ivan", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &away, "away", None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET last_active_at = datetime('now', '-1 hour') WHERE id = ?")
        .bind(&away)
        .execute(&pool)
        .await
        .unwrap();
    let flipped = auth_db::mark_idle_users(&pool, 30 * 60).await.unwrap();
    assert!(flipped.is_empty(), "away rows must not be auto-idled");
    let u = auth_db::find_user_by_id(&pool, &away)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(u.status, "away");
}

// LC-319: auto-expiring custom status.

#[tokio::test]
async fn set_user_status_schedules_expiry_when_modifier_given() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "judy", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "away", Some("brb"), Some("+1 hours"))
        .await
        .unwrap();
    let exp = auth_db::get_custom_status_expiry(&pool, &id).await.unwrap();
    assert!(exp.is_some(), "expected a scheduled expiry timestamp");
    // The text is still present until the sweep runs.
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.custom_status.as_deref(), Some("brb"));
}

#[tokio::test]
async fn set_user_status_clears_expiry_when_modifier_none() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "mallory", "hash")
        .await
        .unwrap();
    auth_db::set_user_status(&pool, &id, "away", Some("brb"), Some("+1 hours"))
        .await
        .unwrap();
    assert!(auth_db::get_custom_status_expiry(&pool, &id)
        .await
        .unwrap()
        .is_some());
    // Re-saving with no modifier removes the expiry (the form is authoritative).
    auth_db::set_user_status(&pool, &id, "away", Some("brb"), None)
        .await
        .unwrap();
    assert!(auth_db::get_custom_status_expiry(&pool, &id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clear_expired_custom_statuses_clears_past_expiry_only() {
    let pool = setup_pool().await;
    let expired = auth_db::create_user(&pool, "oscar", "hash").await.unwrap();
    let future = auth_db::create_user(&pool, "peggy", "hash").await.unwrap();
    let no_expiry = auth_db::create_user(&pool, "trent", "hash").await.unwrap();

    // `away` presence so we can confirm the sweep preserves it.
    auth_db::set_user_status(&pool, &expired, "away", Some("lunch"), Some("+1 hours"))
        .await
        .unwrap();
    auth_db::set_user_status(&pool, &future, "dnd", Some("focus"), Some("+1 hours"))
        .await
        .unwrap();
    auth_db::set_user_status(&pool, &no_expiry, "away", Some("forever"), None)
        .await
        .unwrap();

    // Backdate only the first user's expiry into the past.
    sqlx::query(
        "UPDATE users SET custom_status_expires_at = datetime('now', '-1 minute') WHERE id = ?",
    )
    .bind(&expired)
    .execute(&pool)
    .await
    .unwrap();

    let cleared = auth_db::clear_expired_custom_statuses(&pool).await.unwrap();
    assert_eq!(cleared, vec![(expired.clone(), "away".to_string())]);

    // Expired user: text gone, presence kept.
    let u = auth_db::find_user_by_id(&pool, &expired)
        .await
        .unwrap()
        .unwrap();
    assert!(
        u.custom_status.is_none(),
        "expired custom text must be cleared"
    );
    assert_eq!(u.status, "away", "presence value must be untouched");
    assert!(auth_db::get_custom_status_expiry(&pool, &expired)
        .await
        .unwrap()
        .is_none());

    // Future + no-expiry users untouched.
    let f = auth_db::find_user_by_id(&pool, &future)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.custom_status.as_deref(), Some("focus"));
    let n = auth_db::find_user_by_id(&pool, &no_expiry)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n.custom_status.as_deref(), Some("forever"));
}

#[tokio::test]
async fn clear_expired_custom_statuses_noop_when_nothing_expired() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "victor", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "away", Some("brb"), Some("+1 hours"))
        .await
        .unwrap();
    let cleared = auth_db::clear_expired_custom_statuses(&pool).await.unwrap();
    assert!(cleared.is_empty());
    let u = auth_db::find_user_by_id(&pool, &id).await.unwrap().unwrap();
    assert_eq!(u.custom_status.as_deref(), Some("brb"));
}
