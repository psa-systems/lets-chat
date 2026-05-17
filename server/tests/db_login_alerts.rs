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
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sso_identities.sql"),
        include_str!("../migrations/auth/0017_sso_providers.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn first_combo_is_new_repeat_is_not() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let ua = Some("Firefox/121");
    let ip = Some("10.0.0.1");

    let first = db::login_alerts::check_and_record_device(&pool, &alice, ua, ip)
        .await
        .unwrap();
    assert!(first, "first sighting must be flagged new");

    let second = db::login_alerts::check_and_record_device(&pool, &alice, ua, ip)
        .await
        .unwrap();
    assert!(!second, "second sighting must not re-fire the alert");
}

#[tokio::test]
async fn different_ua_or_ip_counts_as_new_device() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    db::login_alerts::check_and_record_device(&pool, &alice, Some("Firefox/121"), Some("1.1.1.1"))
        .await
        .unwrap();

    let diff_ip = db::login_alerts::check_and_record_device(
        &pool,
        &alice,
        Some("Firefox/121"),
        Some("2.2.2.2"),
    )
    .await
    .unwrap();
    assert!(diff_ip, "different IP is a new device");

    let diff_ua = db::login_alerts::check_and_record_device(
        &pool,
        &alice,
        Some("Chrome/121"),
        Some("1.1.1.1"),
    )
    .await
    .unwrap();
    assert!(diff_ua, "different UA is a new device");
}

#[tokio::test]
async fn scoped_per_user() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let bob = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    let ua = Some("Firefox/121");
    let ip = Some("10.0.0.1");

    let alice_first = db::login_alerts::check_and_record_device(&pool, &alice, ua, ip)
        .await
        .unwrap();
    assert!(alice_first);

    // The same fingerprint must still alert when seen for a different user
    // for the first time. Otherwise an attacker browsing from a popular UA
    // / NATed IP would slip past the alert silently.
    let bob_first = db::login_alerts::check_and_record_device(&pool, &bob, ua, ip)
        .await
        .unwrap();
    assert!(
        bob_first,
        "same fingerprint for a different user must alert"
    );
}

#[tokio::test]
async fn absent_ua_and_ip_does_not_record() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    let flagged = db::login_alerts::check_and_record_device(&pool, &alice, None, None)
        .await
        .unwrap();
    assert!(
        !flagged,
        "with neither UA nor IP captured we have nothing to fingerprint - suppress"
    );
}

#[tokio::test]
async fn ua_only_or_ip_only_still_fingerprints() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    let ua_only = db::login_alerts::check_and_record_device(&pool, &alice, Some("Firefox"), None)
        .await
        .unwrap();
    assert!(ua_only);
    let ua_only_repeat =
        db::login_alerts::check_and_record_device(&pool, &alice, Some("Firefox"), None)
            .await
            .unwrap();
    assert!(!ua_only_repeat);

    let ip_only = db::login_alerts::check_and_record_device(&pool, &alice, None, Some("9.9.9.9"))
        .await
        .unwrap();
    assert!(ip_only);
}

#[tokio::test]
async fn notify_login_alerts_default_is_true() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let rec = db::auth::find_user_by_id(&pool, &alice)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.notify_login_alerts_enabled);
}

#[tokio::test]
async fn setter_round_trip() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::auth::set_notify_login_alerts_enabled(&pool, &alice, false)
        .await
        .unwrap();
    let rec = db::auth::find_user_by_id(&pool, &alice)
        .await
        .unwrap()
        .unwrap();
    assert!(!rec.notify_login_alerts_enabled);
}
