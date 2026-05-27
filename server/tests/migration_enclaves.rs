use sqlx::{Row, SqlitePool};

async fn fresh_pool() -> SqlitePool {
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
        include_str!("../migrations/chat/0041_incoming_webhooks.sql"),
        include_str!("../migrations/chat/0042_outgoing_webhooks.sql"),
        include_str!("../migrations/chat/0043_room_retention.sql"),
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
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn migration_creates_general_and_moves_rooms() {
    let pool = fresh_pool().await;

    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("id");

    let row = sqlx::query("SELECT name, created_by FROM enclaves WHERE id=?")
        .bind(general_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("name"), "General");
    assert_eq!(row.get::<String, _>("created_by"), "system");

    let n: i64 = sqlx::query(
        "SELECT COUNT(*) AS c FROM rooms WHERE enclave_id IS NULL AND room_type != 'dm'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        n, 0,
        "every non-DM room must be in an enclave after migration"
    );

    let m: i64 = sqlx::query(
        "SELECT COUNT(*) AS c FROM rooms WHERE enclave_id = ? AND name IN ('general','random')",
    )
    .bind(general_id)
    .fetch_one(&pool)
    .await
    .unwrap()
    .get("c");
    assert_eq!(m, 2);

    let members: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("c");
    assert_eq!(members, 0, "membership backfill is a separate step");
}

#[tokio::test]
async fn migration_partial_unique_owner_index_enforced() {
    let pool = fresh_pool().await;
    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("id");

    sqlx::query(
        "INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'u1', 'owner')",
    )
    .bind(general_id)
    .execute(&pool)
    .await
    .unwrap();
    let dup = sqlx::query(
        "INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'u2', 'owner')",
    )
    .bind(general_id)
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "two owners per enclave must be rejected");
}
