//! LC-62: unit tests for `db::scheduled` CRUD + dispatcher helpers.
//!
//! Hand-rolled `setup_chat_pool()` per the CLAUDE.md test-maintenance note;
//! the array form keeps a new migration to a one-line append.

use lets_chat::db;
use sqlx::SqlitePool;

async fn setup_chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
        include_str!("../migrations/chat/0009_enclaves.sql"),
        include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
        include_str!("../migrations/chat/0011_threads.sql"),
        include_str!("../migrations/chat/0012_uploads.sql"),
        include_str!("../migrations/chat/0013_link_previews.sql"),
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
        include_str!("../migrations/chat/0016_pinned_messages.sql"),
        include_str!("../migrations/chat/0017_custom_emojis.sql"),
        include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
        include_str!("../migrations/chat/0019_bookmarks.sql"),
        include_str!("../migrations/chat/0020_quote_reply.sql"),
        include_str!("../migrations/chat/0021_enclave_invitations_enclave_idx.sql"),
        include_str!("../migrations/chat/0022_voice_messages.sql"),
        include_str!("../migrations/chat/0023_system_messages.sql"),
        include_str!("../migrations/chat/0024_voice_channel_flag.sql"),
        include_str!("../migrations/chat/0025_message_edits.sql"),
        include_str!("../migrations/chat/0026_room_categories.sql"),
        include_str!("../migrations/chat/0027_user_groups.sql"),
        include_str!("../migrations/chat/0028_room_role_overrides.sql"),
        include_str!("../migrations/chat/0029_room_posting_policy.sql"),
        include_str!("../migrations/chat/0030_room_docs_wiki.sql"),
        include_str!("../migrations/chat/0031_storage_quotas.sql"),
        include_str!("../migrations/chat/0032_anti_spam.sql"),
        include_str!("../migrations/chat/0033_scheduled_messages.sql"),
        include_str!("../migrations/chat/0041_incoming_webhooks.sql"),
        include_str!("../migrations/chat/0042_outgoing_webhooks.sql"),
        include_str!("../migrations/chat/0043_room_retention.sql"),
        include_str!("../migrations/chat/0044_link_filter_quarantine_cascade.sql"),
        include_str!("../migrations/chat/0045_messages_fts_delete_trigger.sql"),
        include_str!("../migrations/chat/0046_messages_fts_purge_guard.sql"),
        include_str!("../migrations/chat/0047_message_drafts.sql"),
        include_str!("../migrations/chat/0048_email_inboxes.sql"),
        include_str!("../migrations/chat/0049_messages_email_inbox_id.sql"),
        include_str!("../migrations/chat/0050_reply_tokens.sql"),
        include_str!("../migrations/chat/0051_processed_message_ids.sql"),
        include_str!("../migrations/chat/0052_remote_control_sessions.sql"),
        include_str!("../migrations/chat/0053_room_feeds.sql"),
        include_str!("../migrations/chat/0054_bridges.sql"),
        include_str!("../migrations/chat/0055_messages_bridge_actor.sql"),
        include_str!("../migrations/chat/0056_bridge_avatar_proxies.sql"),
        include_str!("../migrations/chat/0057_enclave_msg_rate_limit.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn ensure_room(pool: &SqlitePool) -> i64 {
    db::chat::create_room(pool, "general", None, "public", None, None)
        .await
        .unwrap()
}

#[tokio::test]
async fn insert_get_round_trip() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "hello",
        "2099-12-31 23:59:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let row = db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .expect("freshly inserted row");
    assert_eq!(row.id, id);
    assert_eq!(row.room_id, room_id);
    assert_eq!(row.user_id, "u1");
    assert_eq!(row.body, "hello");
    assert_eq!(row.scheduled_for, "2099-12-31 23:59:00");
    assert!(row.delivered_at.is_none());
    assert!(row.failed_reason.is_none());
    assert_eq!(row.attempt_count, 0);
    assert!(row.next_attempt_at.is_none());
}

#[tokio::test]
async fn update_pending_succeeds_then_blocks_after_delivery() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "v1",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let owner_edit =
        db::scheduled::update_scheduled(&pool, id, "u1", "v2", "2099-02-02 00:00:00", None)
            .await
            .unwrap();
    assert!(owner_edit, "owner edit succeeds");
    let row = db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.body, "v2");
    assert_eq!(row.scheduled_for, "2099-02-02 00:00:00");

    let wrong_user =
        db::scheduled::update_scheduled(&pool, id, "other", "evil", "2099-02-02 00:00:00", None)
            .await
            .unwrap();
    assert!(!wrong_user, "wrong-user edit refused");
    let row = db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.body, "v2", "wrong-user edit must not mutate row");

    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let after_delivery =
        db::scheduled::update_scheduled(&pool, id, "u1", "v3", "2099-03-03 00:00:00", None)
            .await
            .unwrap();
    assert!(!after_delivery, "delivered row not editable");
}

#[tokio::test]
async fn delete_pending_succeeds_then_blocks_after_delivery() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "hi",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let wrong_user = db::scheduled::delete_scheduled(&pool, id, "other")
        .await
        .unwrap();
    assert!(!wrong_user, "wrong-user cancel refused");
    assert!(db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .is_some());

    let owner = db::scheduled::delete_scheduled(&pool, id, "u1")
        .await
        .unwrap();
    assert!(owner, "owner cancel succeeds");
    assert!(db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn select_due_filters_by_scheduled_for_and_backoff() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;

    let past = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "past",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let future = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "future",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let cooldown = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "cooldown",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE scheduled_messages SET next_attempt_at = datetime('now', '+10 minutes') \
         WHERE id = ?",
    )
    .bind(cooldown)
    .execute(&pool)
    .await
    .unwrap();
    let delivered = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "done",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(delivered)
        .execute(&pool)
        .await
        .unwrap();

    let due = db::scheduled::select_due(&pool, 100).await.unwrap();
    let ids: Vec<i64> = due.iter().map(|r| r.id).collect();
    assert!(ids.contains(&past), "past-due pending row must surface");
    assert!(!ids.contains(&future), "future row must not surface");
    assert!(!ids.contains(&cooldown), "row in backoff cooldown excluded");
    assert!(!ids.contains(&delivered), "already-delivered row excluded");
}

#[tokio::test]
async fn try_claim_succeeds_once_then_fails() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "h",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut c1 = pool.acquire().await.unwrap();
    let first = db::scheduled::try_claim(&mut c1, id).await.unwrap();
    assert!(first, "first claim wins");
    drop(c1);

    let mut c2 = pool.acquire().await.unwrap();
    let second = db::scheduled::try_claim(&mut c2, id).await.unwrap();
    assert!(!second, "second claim fails: delivered_at already set");
}

#[tokio::test]
async fn delete_for_user_purges_all_states_only_for_target_user() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;

    let pending = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "p",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let delivered = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "d",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(delivered)
        .execute(&pool)
        .await
        .unwrap();
    let dropped = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "x",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE scheduled_messages SET delivered_at = datetime('now'), failed_reason = 'test' \
         WHERE id = ?",
    )
    .bind(dropped)
    .execute(&pool)
    .await
    .unwrap();

    let other = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u2",
        "keep",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let n = db::scheduled::delete_for_user(&mut conn, "u1")
        .await
        .unwrap();
    drop(conn);
    assert_eq!(n, 3, "all three u1 rows removed regardless of state");

    assert!(db::scheduled::get_scheduled(&pool, pending)
        .await
        .unwrap()
        .is_none());
    assert!(db::scheduled::get_scheduled(&pool, delivered)
        .await
        .unwrap()
        .is_none());
    assert!(db::scheduled::get_scheduled(&pool, dropped)
        .await
        .unwrap()
        .is_none());
    assert!(
        db::scheduled::get_scheduled(&pool, other)
            .await
            .unwrap()
            .is_some(),
        "u2 row must survive"
    );
}

#[tokio::test]
async fn list_pending_for_user_orders_and_filters() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;

    let later = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "later",
        "2099-02-02 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let sooner = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "sooner",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let delivered = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "done",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(delivered)
        .execute(&pool)
        .await
        .unwrap();
    let other_user = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u2",
        "hidden",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let pending = db::scheduled::list_pending_for_user(&pool, "u1")
        .await
        .unwrap();
    let ids: Vec<i64> = pending.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![sooner, later],
        "asc by scheduled_for; delivered + other-user excluded"
    );
    assert!(!ids.contains(&other_user));
}

#[tokio::test]
async fn list_recent_dropped_filters_and_limits() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;

    let mut drop_ids = Vec::new();
    for i in 0..3 {
        let id = db::scheduled::insert_scheduled(
            &pool,
            room_id,
            "u1",
            &format!("d{i}"),
            "2000-01-01 00:00:00",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE scheduled_messages \
                SET delivered_at = datetime('now'), failed_reason = ? \
              WHERE id = ?",
        )
        .bind(format!("reason{i}"))
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        drop_ids.push(id);
    }
    let delivered = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "done",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(delivered)
        .execute(&pool)
        .await
        .unwrap();
    let _pending = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "wait",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let recent = db::scheduled::list_recent_dropped_for_user(&pool, "u1", 2)
        .await
        .unwrap();
    assert_eq!(recent.len(), 2, "LIMIT honored");
    for r in &recent {
        assert!(r.failed_reason.is_some(), "only dropped rows surface");
        assert!(drop_ids.contains(&r.id), "only our drops surface");
    }
}

#[tokio::test]
async fn mark_dropped_inside_transaction_records_reason() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "h",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .unwrap();
    let claimed = db::scheduled::try_claim(&mut conn, id).await.unwrap();
    assert!(claimed);
    db::scheduled::mark_dropped(&mut conn, id, "lost room access")
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
    drop(conn);

    let row = db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.delivered_at.is_some(), "claim set delivered_at");
    assert_eq!(row.failed_reason.as_deref(), Some("lost room access"));
}

#[tokio::test]
async fn bump_attempt_increments_and_pushes_cooldown_forward() {
    let pool = setup_chat_pool().await;
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "h",
        "2000-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    db::scheduled::bump_attempt(&pool, id, 120).await.unwrap();
    let row = db::scheduled::get_scheduled(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.attempt_count, 1);
    assert!(row.next_attempt_at.is_some());

    let due = db::scheduled::select_due(&pool, 100).await.unwrap();
    let ids: Vec<i64> = due.iter().map(|r| r.id).collect();
    assert!(
        !ids.contains(&id),
        "row in cooldown excluded from select_due"
    );
}

#[tokio::test]
async fn room_delete_cascades_to_scheduled_rows() {
    let pool = setup_chat_pool().await;
    // CASCADE only fires with foreign keys ON; sqlx-sqlite leaves it OFF
    // by default, so the test enables it explicitly. Production sets the
    // PRAGMA at pool open via SqliteConnectOptions::foreign_keys(true).
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    let room_id = ensure_room(&pool).await;
    let id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "u1",
        "h",
        "2099-01-01 00:00:00",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(room_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        db::scheduled::get_scheduled(&pool, id)
            .await
            .unwrap()
            .is_none(),
        "scheduled row cascade-deletes with its room"
    );
}
