//! LC-78-AVATAR-PROXY: storage helpers for the bridge-avatar cache.

use lets_chat::db::bridge_avatar_proxies as p;

mod common;

#[tokio::test]
async fn upsert_pending_inserts_then_bumps_last_seen() {
    let chat = common::pool("chat").await;
    let inserted_first = p::upsert_pending(&chat, "deadbeef", "https://x.test/a")
        .await
        .unwrap();
    assert!(inserted_first, "first upsert should report a new insert");
    let inserted_again = p::upsert_pending(&chat, "deadbeef", "https://x.test/a")
        .await
        .unwrap();
    assert!(
        !inserted_again,
        "second upsert with the same hash should NOT report a new insert"
    );
    let row = p::find_by_hash(&chat, "deadbeef").await.unwrap().unwrap();
    assert_eq!(row.fetch_status, "pending");
    assert_eq!(row.foreign_url, "https://x.test/a");
}

#[tokio::test]
async fn mark_ok_sets_content_type_and_byte_size() {
    let chat = common::pool("chat").await;
    p::upsert_pending(&chat, "abc", "https://x.test/b")
        .await
        .unwrap();
    let did = p::mark_ok(&chat, "abc", "image/png", 1234).await.unwrap();
    assert!(did);
    let row = p::find_by_hash(&chat, "abc").await.unwrap().unwrap();
    assert_eq!(row.fetch_status, "ok");
    assert_eq!(row.content_type.as_deref(), Some("image/png"));
    assert_eq!(row.byte_size, Some(1234));
    assert!(row.fetched_at.is_some());
    assert!(
        row.failure_reason.is_none(),
        "clean fetch should clear failure_reason"
    );
}

#[tokio::test]
async fn mark_failed_records_reason_truncated() {
    let chat = common::pool("chat").await;
    p::upsert_pending(&chat, "fff", "https://x.test/c")
        .await
        .unwrap();
    // Reason longer than the 256 cap; storage helper truncates.
    let long = "x".repeat(500);
    p::mark_failed(&chat, "fff", &long).await.unwrap();
    let row = p::find_by_hash(&chat, "fff").await.unwrap().unwrap();
    assert_eq!(row.fetch_status, "failed");
    let stored = row.failure_reason.unwrap();
    assert!(
        stored.len() <= 256,
        "failure_reason truncated to <=256 bytes"
    );
}

#[tokio::test]
async fn sweep_unreferenced_skips_referenced_rows() {
    let chat = common::pool("chat").await;
    // Two cache rows. One will be referenced by a message; the other won't.
    p::upsert_pending(&chat, "ref1", "https://x.test/ref1")
        .await
        .unwrap();
    p::upsert_pending(&chat, "orph1", "https://x.test/orph1")
        .await
        .unwrap();
    // Both rows are timestamped to NOW; bump them backwards by 60 days so
    // they fall outside the 30-day retention threshold.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-60 days')")
        .execute(&chat)
        .await
        .unwrap();
    // Need a room + a message referencing `ref1` for the NOT EXISTS check.
    let admin = lets_chat::db::auth::create_user(&common::pool("auth").await, "admin", "h")
        .await
        .unwrap();
    let eid = lets_chat::db::enclave::create_enclave(&chat, "Acme", None, &admin)
        .await
        .unwrap();
    let room = lets_chat::db::chat::create_room(&chat, "r", None, "public", None, Some(eid))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, bridge_foreign_avatar) VALUES (?, '', 'x', 'ref1')",
    )
    .bind(room)
    .execute(&chat)
    .await
    .unwrap();
    let to_delete = p::sweep_unreferenced(&chat, 30).await.unwrap();
    assert_eq!(to_delete, vec!["orph1".to_string()]);
}

#[tokio::test]
async fn sweep_pending_orphans_marks_stale_pending_failed() {
    let chat = common::pool("chat").await;
    p::upsert_pending(&chat, "old", "https://x.test/old")
        .await
        .unwrap();
    p::upsert_pending(&chat, "new", "https://x.test/new")
        .await
        .unwrap();
    // Back-date `old` by 30 minutes; `new` stays at NOW.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-30 minutes') WHERE hash = 'old'")
        .execute(&chat)
        .await
        .unwrap();
    let marked = p::sweep_pending_orphans(&chat, 600).await.unwrap();
    assert_eq!(
        marked, 1,
        "only the 30-min-old pending row should be marked failed"
    );
    assert_eq!(
        p::find_by_hash(&chat, "old")
            .await
            .unwrap()
            .unwrap()
            .fetch_status,
        "failed"
    );
    assert_eq!(
        p::find_by_hash(&chat, "new")
            .await
            .unwrap()
            .unwrap()
            .fetch_status,
        "pending"
    );
}

// LC-207: admin diagnostic helpers --------------------------------------

#[tokio::test]
async fn recent_failures_filters_failed_and_orders_by_last_seen_desc() {
    let chat = common::pool("chat").await;
    // One ok row (must be excluded), two failed rows with distinct last_seen.
    p::upsert_pending(&chat, "okrow", "https://x.test/ok")
        .await
        .unwrap();
    p::mark_ok(&chat, "okrow", "image/png", 10).await.unwrap();
    p::upsert_pending(&chat, "f_old", "https://x.test/fold")
        .await
        .unwrap();
    p::mark_failed(&chat, "f_old", "dns").await.unwrap();
    p::upsert_pending(&chat, "f_new", "https://x.test/fnew")
        .await
        .unwrap();
    p::mark_failed(&chat, "f_new", "http 404").await.unwrap();
    // Make f_old strictly older than f_new by last_seen_at.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-1 hour') WHERE hash = 'f_old'")
        .execute(&chat)
        .await
        .unwrap();

    let failures = p::recent_failures(&chat, 50).await.unwrap();
    let hashes: Vec<&str> = failures.iter().map(|r| r.hash.as_str()).collect();
    assert_eq!(
        hashes,
        vec!["f_new", "f_old"],
        "failed only, most-recently-referenced first; ok row excluded"
    );
}

#[tokio::test]
async fn cache_stats_counts_by_status_sums_bytes_and_flags_stale_pending() {
    let chat = common::pool("chat").await;
    // ok row with bytes; failed row; fresh pending; stale pending.
    p::upsert_pending(&chat, "ok1", "https://x.test/ok1")
        .await
        .unwrap();
    p::mark_ok(&chat, "ok1", "image/png", 1000).await.unwrap();
    p::upsert_pending(&chat, "fail1", "https://x.test/fail1")
        .await
        .unwrap();
    p::mark_failed(&chat, "fail1", "size cap").await.unwrap();
    p::upsert_pending(&chat, "pend_fresh", "https://x.test/pf")
        .await
        .unwrap();
    p::upsert_pending(&chat, "pend_stale", "https://x.test/ps")
        .await
        .unwrap();
    // Back-date pend_stale beyond the 4200s (70-min) display threshold; a row
    // just past the 600s sweep threshold (e.g. 30 min) must NOT count as stale.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-90 minutes') WHERE hash = 'pend_stale'")
        .execute(&chat)
        .await
        .unwrap();

    let s = p::cache_stats(&chat).await.unwrap();
    assert_eq!(s.total, 4);
    assert_eq!(s.ok, 1);
    assert_eq!(s.failed, 1);
    assert_eq!(s.pending, 2);
    assert_eq!(s.pending_stale, 1, "only the 90-min pending row is stale");
    assert_eq!(
        s.total_bytes, 1000,
        "sum is over ok rows only (byte_size NULL elsewhere)"
    );
    assert!(s.oldest_last_seen.is_some());
}

#[tokio::test]
async fn cache_stats_empty_cache_renders_zeros_and_no_oldest() {
    let chat = common::pool("chat").await;
    let s = p::cache_stats(&chat).await.unwrap();
    assert_eq!(s.total, 0);
    assert_eq!(
        s.total_bytes, 0,
        "COALESCE(SUM(...),0) over empty table is 0"
    );
    assert!(
        s.oldest_last_seen.is_none(),
        "MIN over empty table is NULL -> None (template renders '-')"
    );
}

#[tokio::test]
async fn fresh_pending_just_past_sweep_threshold_is_not_stale() {
    let chat = common::pool("chat").await;
    p::upsert_pending(&chat, "p30", "https://x.test/p30")
        .await
        .unwrap();
    // 30 min: past the 600s sweep threshold but inside the 4200s display
    // threshold, so it is normal sweep lag, not an anomaly.
    sqlx::query("UPDATE bridge_avatar_proxies SET last_seen_at = datetime('now', '-30 minutes') WHERE hash = 'p30'")
        .execute(&chat)
        .await
        .unwrap();
    let s = p::cache_stats(&chat).await.unwrap();
    assert_eq!(s.pending_stale, 0);
}
