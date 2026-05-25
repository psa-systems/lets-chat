//! LC-77-SMTP-SEAL: confirms `settings/0006_drop_smtp_settings.sql` clears
//! pre-existing stale SMTP rows from the legacy admin form. Forward-only
//! check: on a fresh deployment there's nothing to clean, but operators
//! who used the earlier UI need their plaintext password row dropped.

mod common;

#[tokio::test]
async fn migration_drops_pre_existing_smtp_settings_rows() {
    // common::settings_pool() runs the FULL migration set including 0006.
    // To exercise the deletion we need to insert the legacy rows AFTER
    // the migration ran, then re-run JUST the 0006 SQL inline (it is
    // idempotent), and confirm the rows are gone. This shape proves
    // 0006's DELETE statement reaches the right keys; it does not
    // require running the migrator from scratch.
    let pool = common::settings_pool().await;

    for (k, v) in [
        ("smtp_host", "smtp.example.com"),
        ("smtp_port", "587"),
        ("smtp_user", "operator"),
        ("smtp_pass", "do-not-leak-me"),
        ("smtp_from", "no-reply@example.com"),
    ] {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
            .bind(k)
            .bind(v)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Sanity: rows were inserted.
    let pre_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM settings \
         WHERE key IN ('smtp_host', 'smtp_port', 'smtp_user', 'smtp_pass', 'smtp_from')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pre_count, 5);

    // Re-run the migration's DELETE statement (the migrator only runs each
    // migration once, but the SQL is idempotent so this is the cleanest
    // way to exercise it inline).
    sqlx::raw_sql(include_str!(
        "../migrations/settings/0006_drop_smtp_settings.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let post_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM settings \
         WHERE key IN ('smtp_host', 'smtp_port', 'smtp_user', 'smtp_pass', 'smtp_from')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(post_count, 0, "all five smtp_* rows must be deleted");

    // Non-SMTP rows are untouched.
    sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('unrelated', 'survives')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/settings/0006_drop_smtp_settings.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let unrelated: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind("unrelated")
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(unrelated.as_deref(), Some("survives"));
}
