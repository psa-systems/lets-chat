use lets_chat::db::push_subscriptions::{self, delete_by_endpoint, for_user, insert_or_replace};
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
    let id = insert_or_replace(
        &pool,
        "u1",
        "https://endpoint.example/abc",
        "p256",
        "auth",
        Some("ua"),
    )
    .await
    .unwrap();
    assert!(id > 0);
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, "https://endpoint.example/abc");
    assert_eq!(subs[0].p256dh_key, "p256");
}

#[tokio::test]
async fn endpoint_conflict_replaces_owning_user() {
    let pool = setup_auth_pool().await;
    insert_or_replace(
        &pool,
        "u1",
        "https://endpoint.example/abc",
        "p1",
        "a1",
        None,
    )
    .await
    .unwrap();
    insert_or_replace(
        &pool,
        "u2",
        "https://endpoint.example/abc",
        "p2",
        "a2",
        None,
    )
    .await
    .unwrap();
    assert!(for_user(&pool, "u1").await.unwrap().is_empty());
    let subs = for_user(&pool, "u2").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].p256dh_key, "p2");
    assert_eq!(subs[0].auth_key, "a2");
}

#[tokio::test]
async fn for_user_lists_all_devices() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u1", "https://e2", "p", "a", None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u2", "https://e3", "p", "a", None)
        .await
        .unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 2);
}

#[tokio::test]
async fn delete_by_endpoint_removes_one_row() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u1", "https://e2", "p", "a", None)
        .await
        .unwrap();
    delete_by_endpoint(&pool, "https://e1").await.unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, "https://e2");
}

#[tokio::test]
async fn register_beyond_cap_evicts_least_recently_seen() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..(cap + 2) {
        insert_or_replace(&pool, "u1", &format!("https://e{i}"), "p", "a", None)
            .await
            .unwrap();
    }
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(
        subs.len(),
        cap,
        "user must be capped at the per-channel max"
    );
    let endpoints: Vec<String> = subs.iter().map(|s| s.endpoint.clone()).collect();
    // The two earliest registrations (lowest ids, same-second last_seen) evict.
    assert!(!endpoints.contains(&"https://e0".to_string()));
    assert!(!endpoints.contains(&"https://e1".to_string()));
    assert!(endpoints.contains(&format!("https://e{}", cap + 1)));
}

#[tokio::test]
async fn reregistering_existing_endpoint_does_not_count_against_cap() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..cap {
        insert_or_replace(&pool, "u1", &format!("https://e{i}"), "p", "a", None)
            .await
            .unwrap();
    }
    // Re-register an existing endpoint: upserts in place, no new row, no eviction.
    insert_or_replace(&pool, "u1", "https://e0", "p2", "a2", None)
        .await
        .unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), cap);
    assert!(subs.iter().any(|s| s.endpoint == "https://e0"));
}

#[tokio::test]
async fn bump_last_seen_updates_timestamp() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None)
        .await
        .unwrap();
    // Rewind last_seen_at so the bump observably differs without sleeping.
    sqlx::query(
        "UPDATE push_subscriptions SET last_seen_at = datetime('now', '-2 seconds') \
          WHERE endpoint = ?",
    )
    .bind("https://e1")
    .execute(&pool)
    .await
    .unwrap();
    let before: String =
        sqlx::query_scalar("SELECT last_seen_at FROM push_subscriptions WHERE endpoint = ?")
            .bind("https://e1")
            .fetch_one(&pool)
            .await
            .unwrap();
    push_subscriptions::bump_last_seen(&pool, "https://e1")
        .await
        .unwrap();
    let after: String =
        sqlx::query_scalar("SELECT last_seen_at FROM push_subscriptions WHERE endpoint = ?")
            .bind("https://e1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(before, after);
}
