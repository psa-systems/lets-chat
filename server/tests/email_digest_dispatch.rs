//! End-to-end test for `email::digest::run_tick` (phase 22 task 4).
//!
//! Seeds a recipient (alice) and a sender (bob), drops a room mention
//! plus a DM into the chat pool, and asserts that one tick produces
//! one outbound email via `MockEmailClient` with the expected subject
//! and body content. Also locks in the "one digest per offline session"
//! gate: a second tick with no new activity must not re-send.

#![cfg(feature = "standalone")]

use lets_chat::db::smtp_settings::{SmtpConfigInput, TlsMode};
use lets_chat::email::digest::{self, DigestConfig};
use lets_chat::email::{EmailClient, MockEmailClient};
use lets_chat::state::AppState;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, push};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-digest-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create tempdir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
            include_str!("../migrations/auth/0007_notification_settings.sql"),
            include_str!("../migrations/auth/0008_two_factor.sql"),
            include_str!("../migrations/auth/0009_push_subscriptions.sql"),
            include_str!("../migrations/auth/0010_digest_columns.sql"),
            include_str!("../migrations/auth/0011_user_email.sql"),
        ],
        "chat" => vec![
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
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
            include_str!("../migrations/settings/0004_smtp_settings.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

/// All the moving parts a digest tick needs: pools, hub, mock email
/// client, alice's user id (the recipient), bob's user id (the
/// sender). The chat migrations auto-seed a "general" room (id 1), so
/// we also pin the id of the room WE created in `room_id` to keep mute
/// assertions on the right row.
struct Harness {
    state: AppState,
    mock: Arc<MockEmailClient>,
    alice_id: String,
    bob_id: String,
    room_id: i64,
}

const TEST_KEY: [u8; 32] = [42u8; 32];

async fn build_harness() -> Harness {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    // Recipient: alice. Set her email, opt her in to digests, and
    // back-date her activity columns so the eligibility predicate
    // accepts her as "offline > 1h ago."
    let alice_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    sqlx::query(
        "UPDATE users \
            SET email = 'alice@example.com', \
                notify_email_digest_enabled = 1, \
                last_active_at = datetime('now', '-2 hours'), \
                last_ws_seen_at = datetime('now', '-2 hours') \
          WHERE id = ?",
    )
    .bind(&alice_id)
    .execute(&auth)
    .await
    .unwrap();

    // Sender: bob. No special configuration.
    let bob_id = db::auth::create_user(&auth, "bob", "hash").await.unwrap();

    // SMTP from address so the tick has something to put in the From
    // header. Password is required to land in the row; AES-encrypted.
    db::smtp_settings::save(
        &settings,
        &TEST_KEY,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: None,
            password: Some("test-pass".into()),
            from_address: "noreply@example.com".into(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();

    // Public site URL for deep links.
    db::settings::set_setting(&settings, "public_base_url", "https://chat.example.com")
        .await
        .unwrap();

    // Seed a public room mention from bob to alice.
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &alice_id)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room_id, &bob_id)
        .await
        .unwrap();
    let msg_id = db::chat::insert_message(&chat, room_id, &bob_id, "Hey @alice can you review?")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO mentions (message_id, room_id, mentioned_user_id, author_user_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(room_id)
    .bind(&alice_id)
    .bind(&bob_id)
    .execute(&chat)
    .await
    .unwrap();

    // Seed a DM from bob to alice.
    let dm = db::chat::create_dm_room(&chat, "alice-bob", &alice_id, &bob_id)
        .await
        .unwrap();
    db::chat::insert_message(&chat, dm.id, &bob_id, "ping me when you are around")
        .await
        .unwrap();

    let mock = Arc::new(MockEmailClient::default());
    let email_client: Arc<dyn EmailClient> = mock.clone();
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        secret_key: Some(Arc::new(TEST_KEY)),
        vapid: None,
        push_client: Arc::new(push::MockPushClient::default()),
        email_client: Some(email_client),
    };
    Harness {
        state,
        mock,
        alice_id,
        bob_id,
        room_id,
    }
}

#[tokio::test]
async fn run_tick_sends_one_digest_combining_mentions_and_dms() {
    let h = build_harness().await;
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    let sent = h.mock.taken();
    assert_eq!(sent.len(), 1, "expected exactly one outbound email");
    let msg = &sent[0];
    assert_eq!(msg.to, "alice@example.com");
    assert_eq!(msg.from, "noreply@example.com");
    assert!(
        msg.subject.contains("1 new mention") && msg.subject.contains("1 direct message"),
        "subject should mention both counts: {:?}",
        msg.subject
    );

    // The plaintext body should contain both snippets and both
    // sections.
    assert!(msg.text_body.contains("Direct messages"));
    assert!(msg.text_body.contains("Mentions"));
    assert!(msg.text_body.contains("ping me when you are around"));
    assert!(msg.text_body.contains("Hey @alice can you review?"));
    // Deep links use the configured base URL.
    assert!(msg.text_body.contains("https://chat.example.com/room/"));
    assert!(msg.text_body.contains("https://chat.example.com/dm/"));

    // The HTML body should bold the mention in the snippet.
    assert!(
        msg.html_body.contains("<strong>@alice</strong>"),
        "expected @alice bolded in html: {:?}",
        msg.html_body
    );

    // Recipient was marked as having received a digest, so the next
    // tick should self-skip her.
    let alice = db::auth::find_user_by_id(&h.state.auth, &h.alice_id)
        .await
        .unwrap()
        .unwrap();
    assert!(alice.last_digest_sent_at.is_some());
}

#[tokio::test]
async fn second_tick_with_no_new_activity_does_not_resend() {
    let h = build_harness().await;
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    assert_eq!(h.mock.taken().len(), 1);

    // Tick again immediately. Nothing new has happened: same activity
    // floor, same last_digest_sent_at. Self-resetting predicate says
    // "not eligible until the user comes back online."
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    assert_eq!(
        h.mock.taken().len(),
        0,
        "expected no second send while user still offline since the first digest"
    );
}

#[tokio::test]
async fn tick_skips_user_who_has_no_email_address() {
    let h = build_harness().await;
    // Wipe the email column to simulate the case where the user has
    // opted in but never set their address.
    sqlx::query("UPDATE users SET email = NULL WHERE id = ?")
        .bind(&h.alice_id)
        .execute(&h.state.auth)
        .await
        .unwrap();
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    assert_eq!(h.mock.taken().len(), 0);
}

#[tokio::test]
async fn tick_skips_user_who_has_not_opted_in() {
    let h = build_harness().await;
    sqlx::query("UPDATE users SET notify_email_digest_enabled = 0 WHERE id = ?")
        .bind(&h.alice_id)
        .execute(&h.state.auth)
        .await
        .unwrap();
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    assert_eq!(h.mock.taken().len(), 0);
}

#[tokio::test]
async fn muting_a_room_excludes_its_mentions_from_the_digest() {
    let h = build_harness().await;
    // Mute the specific room the harness created the mention in.
    // (The chat migrations also auto-seed a "general" room at id 1;
    // muting that one would be a no-op for this test.)
    db::notifications::set_room_mute_mode(
        &h.state.chat,
        &h.alice_id,
        h.room_id,
        db::notifications::MuteMode::All,
    )
    .await
    .unwrap();

    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    let sent = h.mock.taken();
    assert_eq!(sent.len(), 1, "still gets a digest for the DM");
    let msg = &sent[0];
    // The DM section is present; the room mention is not.
    assert!(msg.text_body.contains("Direct messages"));
    assert!(!msg.text_body.contains("Mentions"));
    assert!(!msg.text_body.contains("Hey @alice can you review?"));
}

#[tokio::test]
async fn empty_base_url_still_sends_without_deep_links() {
    let h = build_harness().await;
    // Clear the configured site URL. The tick should still dispatch
    // but the rendered body must omit anchor markup.
    db::settings::set_setting(&h.state.settings, "public_base_url", "")
        .await
        .unwrap();
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    let sent = h.mock.taken();
    assert_eq!(sent.len(), 1);
    let msg = &sent[0];
    assert!(
        !msg.html_body.contains("<a "),
        "no anchor tags expected when site URL empty: {}",
        msg.html_body
    );
    // The snippet content is still there.
    assert!(msg.text_body.contains("ping me when you are around"));
}

#[tokio::test]
async fn tick_no_ops_when_smtp_from_is_unset() {
    let h = build_harness().await;
    // Save SMTP with empty from_address. The tick should bail without
    // attempting any send.
    db::smtp_settings::save(
        &h.state.settings,
        &TEST_KEY,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: None,
            password: None,
            from_address: String::new(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();
    digest::run_tick(&h.state, DigestConfig::default())
        .await
        .unwrap();
    assert_eq!(h.mock.taken().len(), 0);
}
