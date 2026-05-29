use lets_chat::db::apns_subscriptions::{self, delete_by_token, for_user, insert_or_replace};
use sqlx::SqlitePool;

mod common;

async fn setup_auth_pool() -> SqlitePool {
    common::auth_pool().await
}

#[tokio::test]
async fn insert_persists_a_row() {
    let pool = setup_auth_pool().await;
    let id = insert_or_replace(
        &pool,
        "u1",
        "device-token-1",
        Some("com.lc.app"),
        Some("ua"),
    )
    .await
    .unwrap();
    assert!(id > 0);
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].device_token, "device-token-1");
    assert_eq!(subs[0].topic.as_deref(), Some("com.lc.app"));
}

#[tokio::test]
async fn token_conflict_replaces_owning_user() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "tok", Some("t1"), None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u2", "tok", Some("t2"), None)
        .await
        .unwrap();
    assert!(for_user(&pool, "u1").await.unwrap().is_empty());
    let subs = for_user(&pool, "u2").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].topic.as_deref(), Some("t2"));
}

#[tokio::test]
async fn for_user_lists_all_devices() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None, None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u1", "t2", None, None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u2", "t3", None, None)
        .await
        .unwrap();
    assert_eq!(for_user(&pool, "u1").await.unwrap().len(), 2);
}

#[tokio::test]
async fn delete_by_token_removes_one_row() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None, None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u1", "t2", None, None)
        .await
        .unwrap();
    delete_by_token(&pool, "t1").await.unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].device_token, "t2");
}

#[tokio::test]
async fn register_beyond_cap_evicts_least_recently_seen() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..(cap + 2) {
        insert_or_replace(&pool, "u1", &format!("tok{i}"), None, None)
            .await
            .unwrap();
    }
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), cap);
    let tokens: Vec<String> = subs.iter().map(|s| s.device_token.clone()).collect();
    assert!(!tokens.contains(&"tok0".to_string()));
    assert!(!tokens.contains(&"tok1".to_string()));
    assert!(tokens.contains(&format!("tok{}", cap + 1)));
}

#[tokio::test]
async fn reregistering_existing_token_does_not_count_against_cap() {
    let pool = setup_auth_pool().await;
    let cap = lets_chat::db::MAX_PUSH_SUBSCRIPTIONS_PER_USER as usize;
    for i in 0..cap {
        insert_or_replace(&pool, "u1", &format!("tok{i}"), None, None)
            .await
            .unwrap();
    }
    insert_or_replace(&pool, "u1", "tok0", Some("changed"), None)
        .await
        .unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), cap);
    assert!(subs.iter().any(|s| s.device_token == "tok0"));
}

#[tokio::test]
async fn bump_last_seen_updates_timestamp() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "t1", None, None)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE apns_subscriptions SET last_seen_at = datetime('now', '-2 seconds') \
          WHERE device_token = ?",
    )
    .bind("t1")
    .execute(&pool)
    .await
    .unwrap();
    let before: String =
        sqlx::query_scalar("SELECT last_seen_at FROM apns_subscriptions WHERE device_token = ?")
            .bind("t1")
            .fetch_one(&pool)
            .await
            .unwrap();
    apns_subscriptions::bump_last_seen(&pool, "t1")
        .await
        .unwrap();
    let after: String =
        sqlx::query_scalar("SELECT last_seen_at FROM apns_subscriptions WHERE device_token = ?")
            .bind("t1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(before, after);
}
