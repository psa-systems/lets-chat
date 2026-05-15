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
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
        include_str!("../migrations/auth/0012_session_metadata.sql"),
        include_str!("../migrations/auth/0013_digest_columns.sql"),
        include_str!("../migrations/auth/0014_login_alerts.sql"),
        include_str!("../migrations/auth/0015_sso_identities.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn list_returns_only_owners_sessions_with_origin() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let bob = db::auth::create_user(&pool, "bob", "hash").await.unwrap();

    let a1 = db::auth::create_session_with_origin(
        &pool,
        &alice,
        Some("Mozilla/5.0 Firefox/121"),
        Some("10.0.0.1"),
    )
    .await
    .unwrap();
    let _a2 = db::auth::create_session(&pool, &alice).await.unwrap();
    let _b1 = db::auth::create_session_with_origin(&pool, &bob, Some("curl/8.5"), Some("10.0.0.2"))
        .await
        .unwrap();

    let rows = db::auth::list_sessions_for_user(&pool, &alice)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let with_ua = rows.iter().find(|r| r.id == a1).unwrap();
    assert_eq!(
        with_ua.user_agent.as_deref(),
        Some("Mozilla/5.0 Firefox/121")
    );
    assert_eq!(with_ua.ip.as_deref(), Some("10.0.0.1"));
    assert!(
        with_ua.last_seen_at.is_some(),
        "create_session_with_origin must seed last_seen_at"
    );

    let bob_rows = db::auth::list_sessions_for_user(&pool, &bob).await.unwrap();
    assert_eq!(bob_rows.len(), 1);
}

#[tokio::test]
async fn delete_session_for_user_is_user_scoped() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let bob = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    let bob_session = db::auth::create_session(&pool, &bob).await.unwrap();

    // Alice trying to revoke Bob's session must be rejected without
    // touching the row. Otherwise one user could log another out by
    // guessing or replaying a session id.
    let removed = db::auth::delete_session_for_user(&pool, &bob_session, &alice)
        .await
        .unwrap();
    assert!(!removed);
    assert_eq!(
        db::auth::list_sessions_for_user(&pool, &bob)
            .await
            .unwrap()
            .len(),
        1
    );

    let removed = db::auth::delete_session_for_user(&pool, &bob_session, &bob)
        .await
        .unwrap();
    assert!(removed);
    assert!(db::auth::list_sessions_for_user(&pool, &bob)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn expired_sessions_are_filtered_out() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let live = db::auth::create_session(&pool, &alice).await.unwrap();
    let dead = db::auth::create_session(&pool, &alice).await.unwrap();

    // Force one session into the past so list_sessions_for_user must skip it.
    sqlx::query("UPDATE sessions SET expires_at = datetime('now', '-1 day') WHERE id = ?")
        .bind(&dead)
        .execute(&pool)
        .await
        .unwrap();

    let rows = db::auth::list_sessions_for_user(&pool, &alice)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, live);
}

#[tokio::test]
async fn touch_last_seen_updates_column() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let s = db::auth::create_session(&pool, &alice).await.unwrap();

    // Backdate so we can detect that the touch moved the timestamp forward.
    sqlx::query("UPDATE sessions SET last_seen_at = '2000-01-01 00:00:00' WHERE id = ?")
        .bind(&s)
        .execute(&pool)
        .await
        .unwrap();

    db::auth::touch_session_last_seen(&pool, &s).await.unwrap();

    let rows = db::auth::list_sessions_for_user(&pool, &alice)
        .await
        .unwrap();
    let row = rows.iter().find(|r| r.id == s).unwrap();
    let ts = row.last_seen_at.as_deref().unwrap();
    assert_ne!(ts, "2000-01-01 00:00:00");
    assert!(ts.starts_with("20"));
}
