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
