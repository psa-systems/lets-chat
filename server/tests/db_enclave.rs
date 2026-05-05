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
