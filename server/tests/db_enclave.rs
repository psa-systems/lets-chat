use lets_chat::models::enclave::EnclaveRole;
use sqlx::{Row, SqlitePool};

#[test]
fn role_round_trips_via_str() {
    for r in [EnclaveRole::Owner, EnclaveRole::Admin, EnclaveRole::Member] {
        let s = r.as_str();
        assert_eq!(EnclaveRole::from_str(s).unwrap(), r);
    }
    assert!(EnclaveRole::from_str("nope").is_err());
}

async fn auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn insert_user(pool: &SqlitePool, id: &str, username: &str, role: &str, created_at: &str) {
    sqlx::query("INSERT INTO users (id, username, password_hash, role, created_at, updated_at) VALUES (?, ?, 'h', ?, ?, ?)")
        .bind(id).bind(username).bind(role).bind(created_at).bind(created_at)
        .execute(pool).await.unwrap();
}

async fn chat_pool() -> SqlitePool {
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
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn create_enclave_inserts_owner_membership() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "rust", Some("rustaceans"), "u-creator")
        .await
        .unwrap();
    let row = sqlx::query("SELECT name, description, created_by FROM enclaves WHERE id=?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("name"), "rust");
    assert_eq!(
        row.get::<Option<String>, _>("description").unwrap(),
        "rustaceans"
    );
    assert_eq!(row.get::<String, _>("created_by"), "u-creator");

    let role: String =
        sqlx::query("SELECT role FROM enclave_members WHERE enclave_id=? AND user_id=?")
            .bind(id)
            .bind("u-creator")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("role");
    assert_eq!(role, "owner");
}

#[tokio::test]
async fn create_enclave_duplicate_name_errors() {
    let pool = chat_pool().await;
    lets_chat::db::enclave::create_enclave(&pool, "dup", None, "u")
        .await
        .unwrap();
    let err = lets_chat::db::enclave::create_enclave(&pool, "dup", None, "u2").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn get_enclave_round_trip() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u")
        .await
        .unwrap();
    let e = lets_chat::db::enclave::get_enclave(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.name, "x");
    assert!(!e.is_public);
    assert_eq!(e.invite_code, None);
    assert!(lets_chat::db::enclave::get_enclave(&pool, 9999)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn get_membership_returns_role() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u")
        .await
        .unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.role, EnclaveRole::Owner);
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "nobody")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn list_enclaves_for_user_returns_only_member_enclaves() {
    let pool = chat_pool().await;
    let a = lets_chat::db::enclave::create_enclave(&pool, "a", None, "u1")
        .await
        .unwrap();
    let _b = lets_chat::db::enclave::create_enclave(&pool, "b", None, "u2")
        .await
        .unwrap();
    let mine = lets_chat::db::enclave::list_enclaves_for_user(&pool, "u1")
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, a);
}

#[tokio::test]
async fn list_public_enclaves_filters_on_is_public() {
    let pool = chat_pool().await;
    let _ = lets_chat::db::enclave::create_enclave(&pool, "private", None, "u")
        .await
        .unwrap();
    let pub_id = lets_chat::db::enclave::create_enclave(&pool, "open", None, "u")
        .await
        .unwrap();
    sqlx::query("UPDATE enclaves SET is_public=1 WHERE id=?")
        .bind(pub_id)
        .execute(&pool)
        .await
        .unwrap();
    let pubs = lets_chat::db::enclave::list_public_enclaves(&pool)
        .await
        .unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].id, pub_id);
}

#[tokio::test]
async fn add_remove_member_round_trip() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member)
        .await
        .unwrap();
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .is_some());
    lets_chat::db::enclave::remove_member(&pool, id, "u2")
        .await
        .unwrap();
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn add_member_idempotent_via_or_ignore() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member)
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Admin)
        .await
        .unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.role, EnclaveRole::Member, "second add must NOT promote silently");
}

#[tokio::test]
async fn update_role_changes_role() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member)
        .await
        .unwrap();
    lets_chat::db::enclave::update_role(&pool, id, "u2", EnclaveRole::Admin)
        .await
        .unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.role, EnclaveRole::Admin);
}

#[tokio::test]
async fn transfer_ownership_demotes_old_promotes_new_atomically() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Admin)
        .await
        .unwrap();
    lets_chat::db::enclave::transfer_ownership(&pool, id, "u2")
        .await
        .unwrap();
    let prev = lets_chat::db::enclave::get_membership(&pool, id, "owner1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev.role, EnclaveRole::Admin);
    let next = lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.role, EnclaveRole::Owner);
}

#[tokio::test]
async fn transfer_ownership_rejects_non_member() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    let err = lets_chat::db::enclave::transfer_ownership(&pool, id, "stranger").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn create_and_list_invitation() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    let inv_id = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1")
        .await
        .unwrap();
    let pending = lets_chat::db::enclave::list_invitations_for_user(&pool, "u2")
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0.id, inv_id);
    assert_eq!(pending[0].1.id, id);
}

#[tokio::test]
async fn create_invitation_unique_per_invitee_per_enclave() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1")
        .await
        .unwrap();
    let err = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn accept_invitation_inserts_member_and_deletes_invite() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    let inv_id = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1")
        .await
        .unwrap();
    let (eid, uid) = lets_chat::db::enclave::accept_invitation(&pool, inv_id)
        .await
        .unwrap();
    assert_eq!(eid, id);
    assert_eq!(uid, "u2");
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.role, EnclaveRole::Member);
    assert!(lets_chat::db::enclave::list_invitations_for_user(&pool, "u2")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn update_metadata_and_visibility_and_invite_code() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u")
        .await
        .unwrap();
    lets_chat::db::enclave::update_metadata(&pool, id, "y", Some("hi"))
        .await
        .unwrap();
    lets_chat::db::enclave::set_public(&pool, id, true).await.unwrap();
    lets_chat::db::enclave::regenerate_invite_code(&pool, id, "code123")
        .await
        .unwrap();
    let e = lets_chat::db::enclave::get_enclave(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.name, "y");
    assert_eq!(e.description.as_deref(), Some("hi"));
    assert!(e.is_public);
    assert_eq!(e.invite_code.as_deref(), Some("code123"));
    lets_chat::db::enclave::clear_invite_code(&pool, id).await.unwrap();
    let e2 = lets_chat::db::enclave::get_enclave(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e2.invite_code, None);
}

#[tokio::test]
async fn backfill_assigns_owner_admin_member_by_role_and_age() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "admin", "2026-01-01 00:00:00").await;
    insert_user(&auth, "ub", "bob", "moderator", "2026-01-02 00:00:00").await;
    insert_user(&auth, "uc", "carol", "user", "2026-01-03 00:00:00").await;
    insert_user(&auth, "ud", "dave", "admin", "2026-01-04 00:00:00").await;

    lets_chat::db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("id");

    async fn role_of(uid: &str, chat: &SqlitePool) -> String {
        let r: String = sqlx::query("SELECT role FROM enclave_members WHERE enclave_id=(SELECT id FROM enclaves WHERE name='General') AND user_id=?")
            .bind(uid).fetch_one(chat).await.unwrap().get("role");
        r
    }
    assert_eq!(role_of("ua", &chat).await, "owner");
    assert_eq!(role_of("ub", &chat).await, "member");
    assert_eq!(role_of("uc", &chat).await, "member");
    assert_eq!(role_of("ud", &chat).await, "admin");

    let created_by: String = sqlx::query("SELECT created_by FROM enclaves WHERE id=?")
        .bind(general_id)
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("created_by");
    assert_eq!(created_by, "ua");
}

#[tokio::test]
async fn backfill_idempotent() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "admin", "2026-01-01 00:00:00").await;
    insert_user(&auth, "ub", "bob", "user", "2026-01-02 00:00:00").await;
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("c");
    assert_eq!(n, 2);
}

#[tokio::test]
async fn backfill_skips_when_no_admin() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "user", "2026-01-01 00:00:00").await;
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("c");
    assert_eq!(n, 0);
    let cb: String = sqlx::query("SELECT created_by FROM enclaves WHERE name='General'")
        .fetch_one(&chat)
        .await
        .unwrap()
        .get("created_by");
    assert_eq!(cb, "system");
}

#[tokio::test]
async fn delete_enclave_cascades() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u")
        .await
        .unwrap();
    sqlx::query("INSERT INTO rooms (name, room_type, enclave_id) VALUES ('r', 'public', ?)")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    lets_chat::db::enclave::delete_enclave(&pool, id).await.unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM rooms WHERE enclave_id=?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("c");
    assert_eq!(n, 0);
    let m: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members WHERE enclave_id=?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("c");
    assert_eq!(m, 0);
}

#[tokio::test]
async fn list_members_returns_all_with_roles() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1")
        .await
        .unwrap();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'admin1', 'admin')")
        .bind(id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'member1', 'member')")
        .bind(id).execute(&pool).await.unwrap();
    let mut members = lets_chat::db::enclave::list_members(&pool, id)
        .await
        .unwrap();
    members.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    assert_eq!(members.len(), 3);
    assert_eq!(members[0].user_id, "admin1");
    assert_eq!(members[0].role, EnclaveRole::Admin);
}

#[tokio::test]
async fn get_enclave_by_invite_code_finds_match() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u")
        .await
        .unwrap();
    sqlx::query("UPDATE enclaves SET invite_code='abc' WHERE id=?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let e = lets_chat::db::enclave::get_enclave_by_invite_code(&pool, "abc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.id, id);
    assert!(
        lets_chat::db::enclave::get_enclave_by_invite_code(&pool, "missing")
            .await
            .unwrap()
            .is_none()
    );
}
