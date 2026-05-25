//! LC-77 commit 2: schema round-trip for the three new migrations.
//!
//! Asserts:
//! - `settings.imap_inbox_config` table exists with the expected column set.
//! - `chat.email_inboxes` table exists with the expected column set.
//! - `chat.messages.email_inbox_id` column exists.
//! - Inserts and reads back rows in each, confirming end-to-end round-trip.
//! - `db::email_inbox::identity(id)` returns the expected name + avatar_url.

use lets_chat::db;
use sqlx::Row;

mod common;

fn assert_column_present(columns: &[String], name: &str, table: &str) {
    assert!(
        columns.iter().any(|c| c == name),
        "expected column `{name}` on table `{table}`; found columns: {columns:?}"
    );
}

async fn column_names(pool: &sqlx::SqlitePool, table: &str) -> Vec<String> {
    sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .unwrap_or_else(|e| panic!("table_info({table}): {e}"))
        .into_iter()
        .map(|r| r.get::<String, _>("name"))
        .collect()
}

#[tokio::test]
async fn imap_inbox_config_schema_round_trip() {
    let settings = common::settings_pool().await;
    let cols = column_names(&settings, "imap_inbox_config").await;
    for expected in [
        "id",
        "host",
        "port",
        "tls",
        "username",
        "password_encrypted",
        "password_nonce",
        "folder",
        "ingress_domain",
        "enabled",
        "updated_at",
    ] {
        assert_column_present(&cols, expected, "imap_inbox_config");
    }

    // Insert the singleton row (id = 1 CHECK) with placeholder ciphertext +
    // nonce and read it back. AES-256-GCM uses a 12-byte nonce; we don't
    // exercise the cipher here, just the schema's BLOB columns.
    let nonce: [u8; 12] = [0xAA; 12];
    let ciphertext: [u8; 32] = [0xBB; 32];
    sqlx::query(
        "INSERT INTO imap_inbox_config \
         (id, host, port, tls, username, password_encrypted, password_nonce, folder, ingress_domain, enabled) \
         VALUES (1, 'imap.example.com', 993, 1, 'mailer', ?, ?, 'INBOX', 'mail.example.com', 0)",
    )
    .bind(ciphertext.as_slice())
    .bind(nonce.as_slice())
    .execute(&settings)
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT host, port, tls, username, password_encrypted, password_nonce, folder, ingress_domain, enabled \
         FROM imap_inbox_config WHERE id = 1",
    )
    .fetch_one(&settings)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("host"), "imap.example.com");
    assert_eq!(row.get::<i64, _>("port"), 993);
    assert_eq!(row.get::<i64, _>("tls"), 1);
    assert_eq!(row.get::<String, _>("username"), "mailer");
    assert_eq!(row.get::<Vec<u8>, _>("password_encrypted"), ciphertext);
    assert_eq!(row.get::<Vec<u8>, _>("password_nonce"), nonce);
    assert_eq!(row.get::<String, _>("folder"), "INBOX");
    assert_eq!(
        row.get::<Option<String>, _>("ingress_domain"),
        Some("mail.example.com".to_string())
    );
    assert_eq!(row.get::<i64, _>("enabled"), 0);

    // CHECK (id = 1) enforces the singleton invariant; a second insert at
    // id = 2 must fail. Verify so a future migration that drops the CHECK
    // does not silently break the singleton assumption.
    let dup = sqlx::query(
        "INSERT INTO imap_inbox_config \
         (id, host, port, tls, username, password_encrypted, password_nonce, folder, ingress_domain, enabled) \
         VALUES (2, 'imap.other', 993, 1, 'm', ?, ?, 'INBOX', NULL, 0)",
    )
    .bind(ciphertext.as_slice())
    .bind(nonce.as_slice())
    .execute(&settings)
    .await;
    assert!(dup.is_err(), "CHECK (id = 1) must reject id = 2");
}

#[tokio::test]
async fn email_inboxes_schema_round_trip() {
    let chat = common::chat_pool().await;

    let cols = column_names(&chat, "email_inboxes").await;
    for expected in [
        "id",
        "room_id",
        "name",
        "avatar_url",
        "secret_hash",
        "created_by",
        "created_at",
        "last_used_at",
        "revoked_at",
    ] {
        assert_column_present(&cols, expected, "email_inboxes");
    }

    // Create a room to satisfy the FK, then insert an inbox, then read back
    // via the public identity helper. Mirrors the LC-74 webhook insert path.
    let room_id =
        db::chat::create_room(&chat, "ops", None, "public", None, None)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO email_inboxes (room_id, name, avatar_url, secret_hash, created_by) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind("Alerts Bot")
    .bind(Some("https://example.com/avatar.png"))
    .bind("hash-deadbeef")
    .bind("creator-id-001")
    .execute(&chat)
    .await
    .unwrap();

    let identity = db::email_inbox::identity(&chat, 1)
        .await
        .unwrap()
        .expect("identity row");
    assert_eq!(identity.name, "Alerts Bot");
    assert_eq!(
        identity.avatar_url.as_deref(),
        Some("https://example.com/avatar.png")
    );

    // Verify the room FK cascades: dropping the room removes the inbox row,
    // matching the LC-74 webhook table's ON DELETE CASCADE.
    sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(room_id)
        .execute(&chat)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_inboxes")
        .fetch_one(&chat)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "ON DELETE CASCADE must drop email_inboxes");
}

#[tokio::test]
async fn messages_email_inbox_id_column_present_and_round_trips() {
    let chat = common::chat_pool().await;
    let cols = column_names(&chat, "messages").await;
    assert_column_present(&cols, "email_inbox_id", "messages");
    // Confirm the existing webhook_id column also still exists (drift sanity).
    assert_column_present(&cols, "webhook_id", "messages");

    // Insert a synthetic-actor message with email_inbox_id set (mirrors what
    // commit 3's email-ingress insert path will do) and confirm the
    // existing get_message + row_to_raw pipeline returns it.
    let room_id = db::chat::create_room(&chat, "ops", None, "public", None, None)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO email_inboxes (room_id, name, secret_hash, created_by) \
         VALUES (?, 'Inbox A', 'hash-aaaa', 'admin-id')",
    )
    .bind(room_id)
    .execute(&chat)
    .await
    .unwrap();
    let inbox_id: i64 = sqlx::query_scalar("SELECT id FROM email_inboxes WHERE secret_hash = 'hash-aaaa'")
        .fetch_one(&chat)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO messages (room_id, user_id, body, email_inbox_id) \
         VALUES (?, '', 'from email', ?)",
    )
    .bind(room_id)
    .bind(inbox_id)
    .execute(&chat)
    .await
    .unwrap();
    let mid: i64 = sqlx::query_scalar("SELECT id FROM messages WHERE email_inbox_id = ?")
        .bind(inbox_id)
        .fetch_one(&chat)
        .await
        .unwrap();

    let raw = db::chat::get_message(&chat, mid)
        .await
        .unwrap()
        .expect("message row");
    assert_eq!(raw.user_id, "");
    assert_eq!(raw.webhook_id, None);
    assert_eq!(raw.email_inbox_id, Some(inbox_id));

    // ON DELETE SET NULL: dropping the inbox row preserves the message row
    // but nulls the FK column, matching the migration's safety posture.
    sqlx::query("DELETE FROM email_inboxes WHERE id = ?")
        .bind(inbox_id)
        .execute(&chat)
        .await
        .unwrap();
    let after = db::chat::get_message(&chat, mid)
        .await
        .unwrap()
        .expect("message row survives inbox delete");
    assert_eq!(after.email_inbox_id, None);
}
