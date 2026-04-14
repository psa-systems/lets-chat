use std::time::{Duration, Instant};

use lets_chat::server_fns::helpers::{classify_blank_error, STARTUP_GRACE, TRY_AGAIN_ERROR};

#[test]
fn classify_blank_inside_window_returns_try_again() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(30);
    let out = classify_blank_error(now, started_at, "Registration failed");
    assert_eq!(out, TRY_AGAIN_ERROR);
    assert_eq!(TRY_AGAIN_ERROR, "Something went wrong, please try again");
    assert_eq!(STARTUP_GRACE, Duration::from_secs(120));
}

#[test]
fn classify_blank_outside_window_returns_generic() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(121);
    let out = classify_blank_error(now, started_at, "Registration failed");
    assert_eq!(out, "Registration failed");
}

#[test]
fn classify_blank_at_exact_boundary_is_outside_window() {
    let started_at = Instant::now();
    let now = started_at + Duration::from_secs(120);
    let out = classify_blank_error(now, started_at, "Invalid credentials");
    assert_eq!(out, "Invalid credentials");
}

use sqlx::SqlitePool;

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("pool");
    let m1 = include_str!("../migrations/auth/0001_create_tables.sql");
    sqlx::raw_sql(m1).execute(&pool).await.expect("m1");
    let m2 = include_str!("../migrations/auth/0002_read_receipts.sql");
    sqlx::raw_sql(m2).execute(&pool).await.expect("m2");
    pool
}

#[tokio::test]
async fn register_classifier_within_window_returns_try_again() {
    let _ = lets_chat::server_fns::helpers::server_started_at();
    let started_at = lets_chat::server_fns::helpers::server_started_at();
    let out = lets_chat::server_fns::helpers::classify_blank_error(
        std::time::Instant::now(),
        started_at,
        "Registration failed",
    );
    assert_eq!(out, lets_chat::server_fns::helpers::TRY_AGAIN_ERROR);

    let pool = setup_auth_pool().await;
    let count = lets_chat::db::auth::count_users(&pool).await.unwrap();
    assert_eq!(count, 0);
}
