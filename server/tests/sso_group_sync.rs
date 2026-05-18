//! Integration tests for `sso::group_sync::apply`. Uses real auth +
//! chat in-memory pools, seeds enclaves + mappings, then exercises
//! the add / update / remove / no-op branches.

use lets_chat::db::sso_group_mappings as gm;
use lets_chat::db::sso_providers::{self, InsertProvider};
use lets_chat::db::{auth as auth_db, enclave};
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::sso::group_sync;
use sqlx::SqlitePool;

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
            include_str!("../migrations/auth/0010_password_reset.sql"),
            include_str!("../migrations/auth/0011_email_verification.sql"),
            include_str!("../migrations/auth/0012_session_metadata.sql"),
            include_str!("../migrations/auth/0013_digest_columns.sql"),
            include_str!("../migrations/auth/0014_login_alerts.sql"),
            include_str!("../migrations/auth/0015_pending_registrations.sql"),
            include_str!("../migrations/auth/0016_sso_identities.sql"),
            include_str!("../migrations/auth/0017_sso_providers.sql"),
            include_str!("../migrations/auth/0018_sso_flows_provider.sql"),
            include_str!("../migrations/auth/0019_sso_group_mappings.sql"),
            include_str!("../migrations/auth/0020_session_tenant.sql"),
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
            include_str!("../migrations/chat/0020_quote_reply.sql"),
            include_str!("../migrations/chat/0021_enclave_invitations_enclave_idx.sql"),
            include_str!("../migrations/chat/0022_voice_messages.sql"),
            include_str!("../migrations/chat/0023_system_messages.sql"),
            include_str!("../migrations/chat/0024_voice_channel_flag.sql"),
            include_str!("../migrations/chat/0025_message_edits.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn seed_provider(auth: &SqlitePool) {
    sso_providers::insert_provider(
        auth,
        InsertProvider {
            id: "stub",
            kind: "oidc",
            display_name: "Stub",
            issuer_url: "https://idp/",
            client_id: "c",
            client_secret_encrypted: b"s",
            scopes: "openid",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();
}

async fn make_enclave(chat: &SqlitePool, name: &str, creator: &str) -> i64 {
    enclave::create_enclave(chat, name, None, creator)
        .await
        .unwrap()
}

#[tokio::test]
async fn no_mappings_is_a_noop() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "hash").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "hash").await.unwrap();
    let random = make_enclave(&chat, "Random", &admin).await;
    // No provider seeded and no mappings: apply is a no-op even with
    // a group claim in the input list.
    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    assert!(enclave::get_membership(&chat, random, &alice)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn adds_membership_for_mapped_group() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "h").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    let eng = make_enclave(&chat, "Engineering", &admin).await;
    gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();

    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    let m = enclave::get_membership(&chat, eng, &alice)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.role, EnclaveRole::Member);
}

#[tokio::test]
async fn removes_membership_when_group_no_longer_listed() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "h").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    let eng = make_enclave(&chat, "Engineering", &admin).await;
    gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();
    // First sync: alice gets added.
    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    assert!(enclave::get_membership(&chat, eng, &alice)
        .await
        .unwrap()
        .is_some());

    // Second sync without the group: alice gets removed.
    group_sync::apply(&auth, &chat, &alice, "stub", &[])
        .await
        .unwrap();
    assert!(enclave::get_membership(&chat, eng, &alice)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn updates_role_when_mapping_role_changes() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "h").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    let eng = make_enclave(&chat, "Engineering", &admin).await;
    let mid = gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();
    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    assert_eq!(
        enclave::get_membership(&chat, eng, &alice)
            .await
            .unwrap()
            .unwrap()
            .role,
        EnclaveRole::Member
    );

    gm::update_role(&auth, mid, "Admin").await.unwrap();
    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    assert_eq!(
        enclave::get_membership(&chat, eng, &alice)
            .await
            .unwrap()
            .unwrap()
            .role,
        EnclaveRole::Admin
    );
}

#[tokio::test]
async fn higher_role_wins_when_multiple_groups_map_to_same_enclave() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "h").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    let eng = make_enclave(&chat, "Engineering", &admin).await;
    gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();
    gm::insert(&auth, "stub", "engineering-leads", eng, "Admin")
        .await
        .unwrap();
    group_sync::apply(
        &auth,
        &chat,
        &alice,
        "stub",
        &["engineering".into(), "engineering-leads".into()],
    )
    .await
    .unwrap();
    assert_eq!(
        enclave::get_membership(&chat, eng, &alice)
            .await
            .unwrap()
            .unwrap()
            .role,
        EnclaveRole::Admin
    );
}

#[tokio::test]
async fn does_not_demote_an_owner() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    // create_enclave inserts alice as owner.
    let eng = make_enclave(&chat, "Engineering", &alice).await;
    gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();
    group_sync::apply(&auth, &chat, &alice, "stub", &["engineering".into()])
        .await
        .unwrap();
    assert_eq!(
        enclave::get_membership(&chat, eng, &alice)
            .await
            .unwrap()
            .unwrap()
            .role,
        EnclaveRole::Owner,
        "owner role survived sync"
    );

    // Even sync without the group keeps the owner row intact.
    group_sync::apply(&auth, &chat, &alice, "stub", &[])
        .await
        .unwrap();
    assert_eq!(
        enclave::get_membership(&chat, eng, &alice)
            .await
            .unwrap()
            .unwrap()
            .role,
        EnclaveRole::Owner
    );
}

#[tokio::test]
async fn leaves_unrelated_memberships_alone() {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let admin = auth_db::create_user(&auth, "admin", "h").await.unwrap();
    let alice = auth_db::create_user(&auth, "alice", "h").await.unwrap();
    seed_provider(&auth).await;
    let eng = make_enclave(&chat, "Engineering", &admin).await;
    let support = make_enclave(&chat, "Support", &admin).await;
    // Mapping covers Engineering only.
    gm::insert(&auth, "stub", "engineering", eng, "User")
        .await
        .unwrap();
    // Alice is hand-added to Support by an admin (membership outside
    // the scoped set).
    enclave::add_member(&chat, support, &alice, EnclaveRole::Member)
        .await
        .unwrap();

    group_sync::apply(&auth, &chat, &alice, "stub", &[])
        .await
        .unwrap();
    // Support membership untouched (out of scoped set).
    assert!(enclave::get_membership(&chat, support, &alice)
        .await
        .unwrap()
        .is_some());
    // Engineering membership absent (no group claim, in scoped set).
    assert!(enclave::get_membership(&chat, eng, &alice)
        .await
        .unwrap()
        .is_none());
}
