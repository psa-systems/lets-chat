use lets_chat::{db, uploads};
use sqlx::SqlitePool;
use std::sync::OnceLock;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        std::fs::create_dir_all(p.join("uploads")).expect("create uploads subdir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

async fn open_chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: &[&str] = &[
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
        include_str!("../migrations/chat/0046_messages_fts_purge_guard.sql"),
        include_str!("../migrations/chat/0047_message_drafts.sql"),
        include_str!("../migrations/chat/0048_email_inboxes.sql"),
        include_str!("../migrations/chat/0049_messages_email_inbox_id.sql"),
        include_str!("../migrations/chat/0050_reply_tokens.sql"),
    ];
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn insert_orphan(pool: &SqlitePool, storage_path: &str, age_hours: i64) -> i64 {
    let id =
        db::uploads::insert_upload(pool, "user-x", "f.png", "image/png", 10, storage_path, None)
            .await
            .unwrap();
    if age_hours > 0 {
        sqlx::query("UPDATE file_uploads SET created_at = datetime('now', ?) WHERE id = ?")
            .bind(format!("-{age_hours} hours"))
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn insert_linked(pool: &SqlitePool, storage_path: &str, age_hours: i64) -> i64 {
    // Real message_id requires a real room + message. Insert a DM room + a
    // message to give us a valid id, then point the upload at it.
    let room = db::chat::create_dm_room(pool, "dm-sw", "user-x", "user-y")
        .await
        .unwrap();
    let mid = db::chat::insert_message(pool, room.id, "user-x", "msg")
        .await
        .unwrap();
    let upload_id =
        db::uploads::insert_upload(pool, "user-x", "f.png", "image/png", 10, storage_path, None)
            .await
            .unwrap();
    db::uploads::link_upload_to_message(pool, upload_id, mid)
        .await
        .unwrap();
    if age_hours > 0 {
        sqlx::query("UPDATE file_uploads SET created_at = datetime('now', ?) WHERE id = ?")
            .bind(format!("-{age_hours} hours"))
            .bind(upload_id)
            .execute(pool)
            .await
            .unwrap();
    }
    upload_id
}

fn write_file(name: &str) {
    let p = db::uploads_dir().join(name);
    std::fs::write(&p, b"FAKEBYTES").expect("write fixture file");
}

fn file_exists(name: &str) -> bool {
    db::uploads_dir().join(name).exists()
}

async fn row_exists(pool: &SqlitePool, id: i64) -> bool {
    db::uploads::get_upload(pool, id).await.unwrap().is_some()
}

#[tokio::test]
async fn centerpiece_dedup_orphan_sweeps_row_but_keeps_file_for_linked_sibling() {
    ensure_tempdir();
    let pool = open_chat_pool().await;

    // Both rows share storage_path. A is linked; B is orphan past threshold.
    let storage = "centerpiece-shared.png";
    write_file(storage);

    let id_a = insert_linked(&pool, storage, 0).await;
    let id_b = insert_orphan(&pool, storage, 25).await;

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 1);
    assert_eq!(
        stats.files_deleted, 0,
        "file is still referenced by A; must not be deleted"
    );
    assert_eq!(stats.errors, 0);

    assert!(row_exists(&pool, id_a).await, "A's linked row must survive");
    assert!(
        !row_exists(&pool, id_b).await,
        "B's orphan row must be gone"
    );
    assert!(
        file_exists(storage),
        "shared file must survive while A points at it"
    );
}

#[tokio::test]
async fn original_present_preview_absent_sweeps_cleanly() {
    // Pins the Task 3 "preview write failed, original committed" recovery path:
    // an orphan with the original on disk and NO preview file should sweep
    // cleanly (the missing preview is treated as success, not error).
    ensure_tempdir();
    let pool = open_chat_pool().await;

    let storage = "preview-missing-abc.png";
    write_file(storage);
    // intentionally do NOT write the preview

    let id = insert_orphan(&pool, storage, 25).await;
    let preview_name = uploads::preview_storage_name(storage);
    assert!(
        !file_exists(&preview_name),
        "preconditions: preview must be absent"
    );

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 1);
    assert_eq!(
        stats.errors, 0,
        "missing preview must not surface as an error"
    );

    assert!(!row_exists(&pool, id).await);
    assert!(
        !file_exists(storage),
        "original was deleted alongside the row"
    );
}

#[tokio::test]
async fn orphan_younger_than_threshold_survives() {
    ensure_tempdir();
    let pool = open_chat_pool().await;
    let storage = "young-survives.png";
    write_file(storage);
    let id = insert_orphan(&pool, storage, 0).await; // created just now

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 0);
    assert!(row_exists(&pool, id).await);
    assert!(file_exists(storage));
}

#[tokio::test]
async fn linked_row_never_touched_however_old() {
    ensure_tempdir();
    let pool = open_chat_pool().await;
    let storage = "ancient-linked.png";
    write_file(storage);
    let id = insert_linked(&pool, storage, 24 * 100).await; // 100 days old, still linked

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 0);
    assert!(row_exists(&pool, id).await);
    assert!(file_exists(storage));
}

#[tokio::test]
async fn missing_file_on_disk_is_treated_as_success() {
    ensure_tempdir();
    let pool = open_chat_pool().await;
    let storage = "phantom-file.png"; // row exists but no file on disk
    let id = insert_orphan(&pool, storage, 25).await;
    assert!(!file_exists(storage), "preconditions: file must be absent");

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 1);
    assert_eq!(stats.errors, 0);
    assert!(!row_exists(&pool, id).await);
}

#[tokio::test]
async fn preview_file_removed_alongside_original() {
    ensure_tempdir();
    let pool = open_chat_pool().await;
    let storage = "with-preview.png";
    let preview = uploads::preview_storage_name(storage);
    write_file(storage);
    write_file(&preview);

    let id = insert_orphan(&pool, storage, 25).await;

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(stats.rows_deleted, 1);
    assert!(!row_exists(&pool, id).await);
    assert!(!file_exists(storage), "original must be removed");
    assert!(
        !file_exists(&preview),
        "preview must be removed alongside original"
    );
}

#[tokio::test]
async fn orphan_referenced_by_pending_scheduled_message_is_protected() {
    // LC-62: a scheduled message may reference an attachment uploaded days
    // before delivery. The sweep must skip uploads still referenced by a
    // pending scheduled_messages row, and resume sweeping them once the
    // scheduled row is cancelled or delivered.
    ensure_tempdir();
    let pool = open_chat_pool().await;
    let storage = "scheduled-attachment.png";
    write_file(storage);

    let upload_id = insert_orphan(&pool, storage, 25).await;
    let room_id = db::chat::create_room(&pool, "general", None, "public", None, None)
        .await
        .unwrap();
    let sched_id = db::scheduled::insert_scheduled(
        &pool,
        room_id,
        "user-x",
        "with image",
        "2099-01-01 00:00:00",
        None,
        None,
        Some(upload_id),
    )
    .await
    .unwrap();

    let stats = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(
        stats.rows_deleted, 0,
        "upload is shielded by the pending scheduled row"
    );
    assert!(row_exists(&pool, upload_id).await);
    assert!(file_exists(storage));

    let cancelled = db::scheduled::delete_scheduled(&pool, sched_id, "user-x")
        .await
        .unwrap();
    assert!(cancelled);

    let stats2 = uploads::sweep::run_orphan_sweep(&pool, 24).await.unwrap();
    assert_eq!(
        stats2.rows_deleted, 1,
        "upload becomes eligible once protection drops"
    );
    assert!(!row_exists(&pool, upload_id).await);
    assert!(!file_exists(storage));
}
