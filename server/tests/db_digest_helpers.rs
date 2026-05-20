//! Tests for the digest-related auth helpers added in phase 22 task 1:
//! `bump_last_ws_seen` and `set_last_digest_sent_at`, plus the column
//! defaults from `auth/0010_digest_columns.sql`. The throttle around
//! `bump_last_ws_seen` lives at the WS handler call site
//! (`routes/ws.rs`) as a per-connection `Instant` comparison; that is
//! a 3-line predicate verified by inspection rather than by spinning
//! up a real WebSocket here.

use sqlx::{Row, SqlitePool};

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
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sidebar_categories.sql"),
        include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql"),
        include_str!("../migrations/auth/0018_starred_rooms.sql"),
        include_str!("../migrations/auth/0019_api_tokens.sql"),
        include_str!("../migrations/auth/0020_bots.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn new_user_has_null_ws_seen_and_null_digest_sent() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    let row = sqlx::query(
        "SELECT last_ws_seen_at, last_digest_sent_at, notify_email_digest_enabled FROM users WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.get::<Option<String>, _>("last_ws_seen_at").is_none());
    assert!(row
        .get::<Option<String>, _>("last_digest_sent_at")
        .is_none());
    assert_eq!(row.get::<i64, _>("notify_email_digest_enabled"), 0);
}

#[tokio::test]
async fn bump_last_ws_seen_sets_the_column() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();

    lets_chat::db::auth::bump_last_ws_seen(&pool, &id).await;

    let v: Option<String> = sqlx::query("SELECT last_ws_seen_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_ws_seen_at");
    assert!(v.is_some(), "last_ws_seen_at should be non-NULL after bump");
}

#[tokio::test]
async fn bump_last_ws_seen_advances_on_repeat() {
    // The 5-minute per-connection throttle is enforced at the WS handler
    // call site, not inside `bump_last_ws_seen` itself. The DB helper is
    // unconditional: each call writes `datetime('now')`. We back-date the
    // column manually to guarantee the second call observes a strictly
    // newer timestamp on systems where `datetime('now')` does not advance
    // between back-to-back calls.
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    lets_chat::db::auth::bump_last_ws_seen(&pool, &id).await;
    sqlx::query("UPDATE users SET last_ws_seen_at = datetime('now', '-2 minutes') WHERE id = ?")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    let first: String = sqlx::query("SELECT last_ws_seen_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_ws_seen_at");
    lets_chat::db::auth::bump_last_ws_seen(&pool, &id).await;
    let second: String = sqlx::query("SELECT last_ws_seen_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_ws_seen_at");
    assert!(
        second > first,
        "second bump did not advance the timestamp: {first} -> {second}"
    );
}

#[tokio::test]
async fn bump_last_ws_seen_does_not_touch_last_active_at() {
    // The digest design relies on these two timestamps moving independently:
    // `last_active_at` for HTTP-request activity (drives idle-flip),
    // `last_ws_seen_at` for "in-app surface alive" (drives digest gating).
    // Confirm the WS-bump does not write to `last_active_at`.
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET last_active_at = datetime('now', '-2 hours') WHERE id = ?")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    let before: String = sqlx::query("SELECT last_active_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_active_at");
    lets_chat::db::auth::bump_last_ws_seen(&pool, &id).await;
    let after: String = sqlx::query("SELECT last_active_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_active_at");
    assert_eq!(before, after, "WS bump must not touch last_active_at");
}

#[tokio::test]
async fn set_last_digest_sent_at_sets_the_column() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "hash")
        .await
        .unwrap();
    lets_chat::db::auth::set_last_digest_sent_at(&pool, &id)
        .await
        .unwrap();
    let v: Option<String> = sqlx::query("SELECT last_digest_sent_at FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("last_digest_sent_at");
    assert!(
        v.is_some(),
        "last_digest_sent_at should be non-NULL after set"
    );
}
