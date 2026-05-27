use lets_chat::db::fcm_subscriptions::{self, delete_by_token, for_user, insert_or_replace};
use sqlx::SqlitePool;

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
        include_str!("../migrations/auth/0008_two_factor.sql"),
        include_str!("../migrations/auth/0009_push_subscriptions.sql"),
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
        include_str!("../migrations/auth/0012_session_metadata.sql"),
        include_str!("../migrations/auth/0013_digest_columns.sql"),
        include_str!("../migrations/auth/0014_login_alerts.sql"),
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sidebar_categories.sql"),
        include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql"),
        include_str!("../migrations/auth/0018_starred_rooms.sql"),
        include_str!("../migrations/auth/0019_api_tokens.sql"),
        include_str!("../migrations/auth/0020_bots.sql"),
        include_str!("../migrations/auth/0021_user_dnd.sql"),
        include_str!("../migrations/auth/0022_mobile_push.sql"),
        include_str!("../migrations/auth/0023_notify_email_activity.sql"),
        include_str!("../migrations/auth/0024_user_locale.sql"),
        include_str!("../migrations/auth/0025_user_theme.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn insert_persists_a_row() {
    let pool = setup_auth_pool().await;
    let id = insert_or_replace(&pool, "u1", "reg-token-1", Some("ua"))
        .await
        .unwrap();
    assert!(id > 0);
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].registration_token, "reg-token-1");
}

#[tokio::test]
async fn token_conflict_replaces_owning_user() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "tok", None).await.unwrap();
    insert_or_replace(&pool, "u2", "tok", None).await.unwrap();
    assert!(for_user(&pool, "u1").await.unwrap().is_empty());
    assert_eq!(for_user(&pool, "u2").await.unwrap().len(), 1);
}

#[tokio::test]
async fn for_user_lists_all_devices() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None).await.unwrap();
    insert_or_replace(&pool, "u1", "t2", None).await.unwrap();
    insert_or_replace(&pool, "u2", "t3", None).await.unwrap();
    assert_eq!(for_user(&pool, "u1").await.unwrap().len(), 2);
}

#[tokio::test]
async fn delete_by_token_removes_one_row() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None).await.unwrap();
    insert_or_replace(&pool, "u1", "t2", None).await.unwrap();
    delete_by_token(&pool, "t1").await.unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].registration_token, "t2");
}

#[tokio::test]
async fn register_beyond_cap_evicts_least_recently_seen() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..(cap + 2) {
        insert_or_replace(&pool, "u1", &format!("tok{i}"), None)
            .await
            .unwrap();
    }
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), cap);
    let tokens: Vec<String> = subs.iter().map(|s| s.registration_token.clone()).collect();
    assert!(!tokens.contains(&"tok0".to_string()));
    assert!(!tokens.contains(&"tok1".to_string()));
    assert!(tokens.contains(&format!("tok{}", cap + 1)));
}

#[tokio::test]
async fn reregistering_existing_token_does_not_count_against_cap() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..cap {
        insert_or_replace(&pool, "u1", &format!("tok{i}"), None)
            .await
            .unwrap();
    }
    insert_or_replace(&pool, "u1", "tok0", Some("changed"))
        .await
        .unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), cap);
    assert!(subs.iter().any(|s| s.registration_token == "tok0"));
}

#[tokio::test]
async fn bump_last_seen_updates_timestamp() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None).await.unwrap();
    sqlx::query(
        "UPDATE fcm_subscriptions SET last_seen_at = datetime('now', '-2 seconds') \
          WHERE registration_token = ?",
    )
    .bind("t1")
    .execute(&pool)
    .await
    .unwrap();
    let before: String = sqlx::query_scalar(
        "SELECT last_seen_at FROM fcm_subscriptions WHERE registration_token = ?",
    )
    .bind("t1")
    .fetch_one(&pool)
    .await
    .unwrap();
    fcm_subscriptions::bump_last_seen(&pool, "t1")
        .await
        .unwrap();
    let after: String = sqlx::query_scalar(
        "SELECT last_seen_at FROM fcm_subscriptions WHERE registration_token = ?",
    )
    .bind("t1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(before, after);
}
