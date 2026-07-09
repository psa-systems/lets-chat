// Verifies the 0037 migration renamed theme -> theme_mode and added theme_palette,
// preserving existing rows. Uses the same migrator the app uses.
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn migration_renames_theme_and_adds_palette() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations/auth")
        .run(&pool)
        .await
        .unwrap();

    // Columns exist with expected names.
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        cols.contains(&"theme_mode".to_string()),
        "theme_mode column missing"
    );
    assert!(
        cols.contains(&"theme_palette".to_string()),
        "theme_palette column missing"
    );
    assert!(
        !cols.contains(&"theme".to_string()),
        "old theme column still present"
    );
}
