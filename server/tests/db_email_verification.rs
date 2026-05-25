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
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn token_round_trip_and_single_use() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();

    let token = db::email_verification::create_token(&pool, &id, "alice@example.com")
        .await
        .unwrap();
    assert!(token.len() >= 32);

    let active = db::email_verification::find_active(&pool, &token)
        .await
        .unwrap()
        .expect("token should be active");
    assert_eq!(active.user_id, id);
    assert_eq!(active.email, "alice@example.com");

    let consumed = db::email_verification::mark_used(&pool, &token)
        .await
        .unwrap();
    assert_eq!(consumed, 1);

    let active = db::email_verification::find_active(&pool, &token)
        .await
        .unwrap();
    assert!(active.is_none(), "used token must not validate again");

    let second = db::email_verification::mark_used(&pool, &token)
        .await
        .unwrap();
    assert_eq!(second, 0);
}

#[tokio::test]
async fn unknown_token_does_not_match() {
    let pool = setup_pool().await;
    let active = db::email_verification::find_active(&pool, "bogus-token")
        .await
        .unwrap();
    assert!(active.is_none());
}

#[tokio::test]
async fn invalidate_all_burns_outstanding_tokens() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();
    let t1 = db::email_verification::create_token(&pool, &id, "alice@example.com")
        .await
        .unwrap();
    let t2 = db::email_verification::create_token(&pool, &id, "alice@example.com")
        .await
        .unwrap();

    db::email_verification::invalidate_all_for_user(&pool, &id)
        .await
        .unwrap();

    assert!(db::email_verification::find_active(&pool, &t1)
        .await
        .unwrap()
        .is_none());
    assert!(db::email_verification::find_active(&pool, &t2)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn mark_email_verified_requires_email_match() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();

    let stale = db::auth::mark_email_verified(&pool, &id, "old@example.com")
        .await
        .unwrap();
    assert_eq!(stale, 0, "token issued for an old address must not verify");
    assert!(db::auth::get_user_email_verified_at(&pool, &id)
        .await
        .unwrap()
        .is_none());

    let ok = db::auth::mark_email_verified(&pool, &id, "alice@example.com")
        .await
        .unwrap();
    assert_eq!(ok, 1);
    assert!(db::auth::get_user_email_verified_at(&pool, &id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn changing_email_clears_verified_at_but_resave_does_not() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();
    db::auth::mark_email_verified(&pool, &id, "alice@example.com")
        .await
        .unwrap();
    assert!(db::auth::get_user_email_verified_at(&pool, &id)
        .await
        .unwrap()
        .is_some());

    // Re-saving the same address must not strip the verified state.
    db::auth::set_user_email(&pool, &id, Some("alice@example.com"))
        .await
        .unwrap();
    assert!(
        db::auth::get_user_email_verified_at(&pool, &id)
            .await
            .unwrap()
            .is_some(),
        "re-save of same email must keep verification"
    );

    // Switching to a new address clears verification.
    db::auth::set_user_email(&pool, &id, Some("alice2@example.com"))
        .await
        .unwrap();
    assert!(
        db::auth::get_user_email_verified_at(&pool, &id)
            .await
            .unwrap()
            .is_none(),
        "changing email must drop verification"
    );

    // Clearing the address also drops verification.
    db::auth::mark_email_verified(&pool, &id, "alice2@example.com")
        .await
        .unwrap();
    db::auth::set_user_email(&pool, &id, None).await.unwrap();
    assert!(db::auth::get_user_email_verified_at(&pool, &id)
        .await
        .unwrap()
        .is_none());
}
