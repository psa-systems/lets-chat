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
    auth_db::set_user_status(&pool, &id, "dnd", Some("debugging"))
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
    let err = auth_db::set_user_status(&pool, &id, "offline", None)
        .await
        .unwrap_err();
    assert!(matches!(err, auth_db::SetStatusError::InvalidStatus));
}

#[tokio::test]
async fn set_user_status_rejects_overlong_custom() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "bob", "hash").await.unwrap();
    let too_long = "x".repeat(51);
    let err = auth_db::set_user_status(&pool, &id, "active", Some(&too_long))
        .await
        .unwrap_err();
    assert!(matches!(err, auth_db::SetStatusError::CustomTooLong(50)));
}

#[tokio::test]
async fn touch_user_activity_flips_idle_back_to_active() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "carol", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "idle", None)
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
    auth_db::set_user_status(&pool, &id, "dnd", Some("focus"))
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
    auth_db::set_user_status(&pool, &dnd, "dnd", None)
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
    auth_db::set_user_status(&pool, &id, "dnd", None)
        .await
        .unwrap();
    assert!(auth_db::is_user_dnd(&pool, &id).await.unwrap());
}

#[tokio::test]
async fn set_user_status_accepts_away() {
    let pool = setup_pool().await;
    let id = auth_db::create_user(&pool, "grace", "hash").await.unwrap();
    auth_db::set_user_status(&pool, &id, "away", Some("lunch"))
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
    auth_db::set_user_status(&pool, &id, "away", None)
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
    auth_db::set_user_status(&pool, &away, "away", None)
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
