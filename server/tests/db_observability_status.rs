//! LC-207-OBSERVABILITY (#278): db-layer round-trip tests for the three
//! status surfaces - IMAP poll health, email-ingress drop log, retention sweep
//! status. Uses the common `sqlx::migrate!`-backed pools so new migrations are
//! picked up automatically.

mod common;

use lets_chat::db;

#[tokio::test]
async fn imap_poll_status_success_then_failure_then_recover() {
    let settings = common::settings_pool().await;

    // No row until the first tick records one.
    assert!(db::imap_poll_status::read(&settings)
        .await
        .unwrap()
        .is_none());

    db::imap_poll_status::record_success(&settings, 5, 3, 2)
        .await
        .unwrap();
    let s = db::imap_poll_status::read(&settings)
        .await
        .unwrap()
        .expect("status row");
    assert!(s.last_poll_at.is_some());
    assert!(s.last_ok_at.is_some());
    assert_eq!(s.last_error, None);
    assert_eq!(s.consecutive_failures, 0);
    assert_eq!((s.last_fetched, s.last_posted, s.last_dropped), (5, 3, 2));

    // Two failures in a row: error set, counter climbs, last_ok_at preserved.
    db::imap_poll_status::record_failure(&settings, "LOGIN failed")
        .await
        .unwrap();
    db::imap_poll_status::record_failure(&settings, "connect refused")
        .await
        .unwrap();
    let s = db::imap_poll_status::read(&settings)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.consecutive_failures, 2);
    assert_eq!(s.last_error.as_deref(), Some("connect refused"));
    assert!(
        s.last_ok_at.is_some(),
        "last success time must survive failures"
    );

    // Recovery resets the counter and clears the error.
    db::imap_poll_status::record_success(&settings, 1, 1, 0)
        .await
        .unwrap();
    let s = db::imap_poll_status::read(&settings)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.consecutive_failures, 0);
    assert_eq!(s.last_error, None);
}

#[tokio::test]
async fn email_ingress_drops_record_recent_counts_and_sweep() {
    let chat = common::chat_pool().await;

    db::email_ingress_drops::record(&chat, "rate_limited", Some(11), "inbox 4, retry 30s")
        .await
        .unwrap();
    db::email_ingress_drops::record(&chat, "parse_fail", Some(12), "")
        .await
        .unwrap();
    db::email_ingress_drops::record(&chat, "rate_limited", Some(13), "inbox 4, retry 30s")
        .await
        .unwrap();

    // recent() is newest-first.
    let recent = db::email_ingress_drops::recent(&chat, 10).await.unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0].uid, Some(13));
    assert_eq!(recent[0].reason, "rate_limited");
    // Empty detail stored as NULL.
    let parse = recent.iter().find(|d| d.reason == "parse_fail").unwrap();
    assert_eq!(parse.detail, None);

    // counts_by_reason groups within the window, highest first.
    let counts = db::email_ingress_drops::counts_by_reason(&chat, 24)
        .await
        .unwrap();
    assert_eq!(counts[0], ("rate_limited".to_string(), 2));
    assert!(counts.iter().any(|(r, n)| r == "parse_fail" && *n == 1));

    // sweep_old drops only rows past the horizon. Back-date one row 40 days.
    sqlx::query(
        "UPDATE email_ingress_drops SET dropped_at = datetime('now', '-40 days') WHERE uid = 12",
    )
    .execute(&chat)
    .await
    .unwrap();
    let swept = db::email_ingress_drops::sweep_old(&chat, 30).await.unwrap();
    assert_eq!(swept, 1, "only the back-dated row should be swept");
    assert_eq!(
        db::email_ingress_drops::recent(&chat, 10)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn email_ingress_drop_detail_is_truncated() {
    let chat = common::chat_pool().await;
    let long = "x".repeat(500);
    db::email_ingress_drops::record(&chat, "internal_error", None, &long)
        .await
        .unwrap();
    let recent = db::email_ingress_drops::recent(&chat, 1).await.unwrap();
    let detail = recent[0].detail.as_deref().unwrap();
    assert_eq!(
        detail.chars().count(),
        200,
        "detail must be capped at 200 chars"
    );
}

#[tokio::test]
async fn retention_status_accumulates_runs_and_records_error() {
    let chat = common::chat_pool().await;

    assert!(db::retention_status::read(&chat).await.unwrap().is_none());

    // Two completed runs: lifetime total accumulates, runs increments, the
    // last-run snapshot reflects the most recent tick (including a 0-delete).
    db::retention_status::record_run(&chat, 2, 10)
        .await
        .unwrap();
    db::retention_status::record_run(&chat, 0, 0).await.unwrap();
    let s = db::retention_status::read(&chat).await.unwrap().unwrap();
    assert_eq!(s.runs, 2);
    assert_eq!(s.total_messages_deleted, 10);
    assert_eq!(s.last_messages_deleted, 0);
    assert_eq!(s.last_rooms_touched, 0);
    assert!(s.last_run_at.is_some());
    assert_eq!(s.last_error, None);

    // A failure records the error without bumping runs or touching last_run_at.
    let before = s.last_run_at.clone();
    db::retention_status::record_error(&chat, "database is locked")
        .await
        .unwrap();
    let s = db::retention_status::read(&chat).await.unwrap().unwrap();
    assert_eq!(s.runs, 2, "a failed tick is not a completed run");
    assert_eq!(s.last_error.as_deref(), Some("database is locked"));
    assert_eq!(s.last_run_at, before, "failure must not move last_run_at");

    // A subsequent success clears the error.
    db::retention_status::record_run(&chat, 1, 4).await.unwrap();
    let s = db::retention_status::read(&chat).await.unwrap().unwrap();
    assert_eq!(s.last_error, None);
    assert_eq!(s.total_messages_deleted, 14);
    assert_eq!(s.runs, 3);
}
