use lets_chat::db;
use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
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
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn email_round_trip_and_lookup_is_case_insensitive() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    assert_eq!(db::auth::get_user_email(&pool, &id).await.unwrap(), None);

    db::auth::set_user_email(&pool, &id, Some("Alice@Example.com"))
        .await
        .unwrap();
    assert_eq!(
        db::auth::get_user_email(&pool, &id)
            .await
            .unwrap()
            .as_deref(),
        Some("Alice@Example.com")
    );

    let found = db::auth::find_user_id_by_email(&pool, "alice@example.com")
        .await
        .unwrap();
    assert_eq!(found.as_deref(), Some(id.as_str()));

    let upper = db::auth::find_user_id_by_email(&pool, "ALICE@EXAMPLE.COM")
        .await
        .unwrap();
    assert_eq!(upper.as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn duplicate_email_rejected() {
    let pool = setup_pool().await;
    let a = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let b = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &a, Some("shared@example.com"))
        .await
        .unwrap();
    let err = db::auth::set_user_email(&pool, &b, Some("shared@example.com"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, sqlx::Error::Database(d) if d.is_unique_violation()),
        "expected unique violation, got {err:?}"
    );
}

#[tokio::test]
async fn banned_users_excluded_from_email_lookup() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();
    db::auth::ban_user(&pool, &id, Some("spam")).await.unwrap();
    let found = db::auth::find_user_id_by_email(&pool, "alice@example.com")
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn create_token_round_trip_and_mark_used_single_shot() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let token = db::password_reset::create_token(&pool, &id).await.unwrap();
    assert!(token.len() >= 32, "token should be at least 32 chars");

    let active = db::password_reset::find_active_user_id(&pool, &token)
        .await
        .unwrap();
    assert_eq!(active.as_deref(), Some(id.as_str()));

    let consumed = db::password_reset::mark_used(&pool, &token).await.unwrap();
    assert_eq!(consumed, 1);

    let active = db::password_reset::find_active_user_id(&pool, &token)
        .await
        .unwrap();
    assert!(active.is_none(), "used token must not validate again");

    let second = db::password_reset::mark_used(&pool, &token).await.unwrap();
    assert_eq!(second, 0, "marking already-used token returns 0 rows");
}

#[tokio::test]
async fn unknown_token_does_not_match() {
    let pool = setup_pool().await;
    let active = db::password_reset::find_active_user_id(&pool, "not-a-real-token")
        .await
        .unwrap();
    assert!(active.is_none());
}

#[tokio::test]
async fn invalidate_all_burns_concurrent_tokens() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let t1 = db::password_reset::create_token(&pool, &id).await.unwrap();
    let t2 = db::password_reset::create_token(&pool, &id).await.unwrap();

    db::password_reset::invalidate_all_for_user(&pool, &id)
        .await
        .unwrap();

    assert!(
        db::password_reset::find_active_user_id(&pool, &t1)
            .await
            .unwrap()
            .is_none(),
        "first token should be invalidated"
    );
    assert!(
        db::password_reset::find_active_user_id(&pool, &t2)
            .await
            .unwrap()
            .is_none(),
        "second token should be invalidated"
    );
}

#[tokio::test]
async fn set_password_hash_changes_login_credential() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "old-hash")
        .await
        .unwrap();
    db::auth::set_password_hash(&pool, &id, "new-hash")
        .await
        .unwrap();
    let user = db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.password_hash, "new-hash");
}
