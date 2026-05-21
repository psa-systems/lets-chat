//! Phase 21 scale sanity check (gated behind `#[ignore]`).
//!
//! Seeds 10,000 mention rows for a single user across 50 rooms and times the
//! two hot read/write queries the broadcast-mention path will exercise heavily.
//! Budget: each query under 50ms on the in-memory SQLite test fixture. The
//! budget is loose - we care about "not orders of magnitude slower than the
//! per-`@username` baseline," not microsecond precision.
//!
//! Run with:
//!   ./dev/cargo test -p lets-chat-server --test scale_mentions \
//!       -- --ignored --nocapture

use sqlx::SqlitePool;
use std::time::Instant;

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
        include_str!("../migrations/chat/0014_mentions.sql"),
        include_str!("../migrations/chat/0015_room_notification_settings.sql"),
        include_str!("../migrations/chat/0016_pinned_messages.sql"),
        include_str!("../migrations/chat/0017_custom_emojis.sql"),
        include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
        include_str!("../migrations/chat/0019_bookmarks.sql"),
        include_str!("../migrations/chat/0032_anti_spam.sql"),
        include_str!("../migrations/chat/0034_branding.sql"),
        include_str!("../migrations/chat/0035_analytics_daily.sql"),
        include_str!("../migrations/chat/0036_branding_favicon.sql"),
        include_str!("../migrations/chat/0037_reminders.sql"),
        include_str!("../migrations/chat/0038_polls.sql"),
        include_str!("../migrations/chat/0039_slash_commands_custom.sql"),
        include_str!("../migrations/chat/0040_enclave_last_room.sql"),
        include_str!("../migrations/chat/0041_incoming_webhooks.sql"),
        include_str!("../migrations/chat/0042_outgoing_webhooks.sql"),
        include_str!("../migrations/chat/0043_room_retention.sql"),
        include_str!("../migrations/chat/0044_link_filter_quarantine_cascade.sql"),
        include_str!("../migrations/chat/0045_messages_fts_delete_trigger.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
#[ignore]
async fn scale_mention_queries_under_50ms_at_10k_rows() {
    let pool = setup_chat_pool().await;

    let user_id = "scale-user-1";
    let n_rooms: i64 = 50;
    let rows_per_room: i64 = 200;
    let total_rows = n_rooms * rows_per_room;
    // Start above the migration-seeded room ids (general=1, random=2 from
    // 0001_create_tables.sql).
    let room_id_base: i64 = 100;

    // Seed rooms (required by the FK), one author message per room (required
    // by the FK on mentions.message_id), and N mention rows per room.
    // Bulk-insert via a single transaction to keep setup fast.
    let mut tx = pool.begin().await.unwrap();
    for i in 0..n_rooms {
        let room_id = room_id_base + i;
        sqlx::query(
            "INSERT INTO rooms (id, name, room_type, created_at) \
             VALUES (?, ?, 'public', datetime('now'))",
        )
        .bind(room_id)
        .bind(format!("scale-room-{room_id}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // Seed one message per room with a synthetic id so the mention FK is happy.
    for i in 0..n_rooms {
        let room_id = room_id_base + i;
        sqlx::query(
            "INSERT INTO messages (id, room_id, user_id, body, created_at) \
             VALUES (?, ?, 'author-1', 'seed', datetime('now'))",
        )
        .bind(room_id)
        .bind(room_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // Seed mentions: rows_per_room mentions per room, all for `user_id`,
    // all unread (read_at = NULL). Each row needs a unique message_id to
    // satisfy the UNIQUE (message_id, mentioned_user_id) constraint, so we
    // synthesize extra message rows.
    let mut next_msg_id: i64 = room_id_base + n_rooms;
    for i in 0..n_rooms {
        let room_id = room_id_base + i;
        for _ in 0..rows_per_room {
            sqlx::query(
                "INSERT INTO messages (id, room_id, user_id, body, created_at) \
                 VALUES (?, ?, 'author-1', 'seed', datetime('now'))",
            )
            .bind(next_msg_id)
            .bind(room_id)
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO mentions \
                    (message_id, room_id, mentioned_user_id, author_user_id, created_at) \
                 VALUES (?, ?, ?, 'author-1', datetime('now'))",
            )
            .bind(next_msg_id)
            .bind(room_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .unwrap();
            next_msg_id += 1;
        }
    }
    tx.commit().await.unwrap();

    // Sanity: 10K mention rows for this user.
    let actual: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mentions WHERE mentioned_user_id = ?")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actual, total_rows, "seed: expected {total_rows} rows");

    // Time count_unread_mentions_per_room: the sidebar query, runs on every
    // page render. This is the most-touched read path.
    let t0 = Instant::now();
    let counts = lets_chat::db::mentions::count_unread_mentions_per_room(&pool, user_id)
        .await
        .unwrap();
    let elapsed_count = t0.elapsed();
    assert_eq!(
        counts.len() as i64,
        n_rooms,
        "expected one row per seeded room"
    );

    // Time mark_mentions_read_for_room: the write path that flips read_at,
    // runs on every "I opened this room" event. Use a watermark high enough
    // to flip everything in one of the seeded rooms.
    let target_room: i64 = room_id_base;
    let high_watermark: i64 = i64::MAX;
    let t1 = Instant::now();
    let flipped = lets_chat::db::mentions::mark_mentions_read_for_room(
        &pool,
        user_id,
        target_room,
        high_watermark,
    )
    .await
    .unwrap();
    let elapsed_mark = t1.elapsed();
    assert_eq!(flipped, rows_per_room as u64);

    println!(
        "scale_mentions: rows={total_rows}, \
         count_unread_mentions_per_room={elapsed_count:?}, \
         mark_mentions_read_for_room={elapsed_mark:?}"
    );

    let budget = std::time::Duration::from_millis(50);
    assert!(
        elapsed_count < budget,
        "count_unread_mentions_per_room took {elapsed_count:?}, budget {budget:?}"
    );
    assert!(
        elapsed_mark < budget,
        "mark_mentions_read_for_room took {elapsed_mark:?}, budget {budget:?}"
    );
}
