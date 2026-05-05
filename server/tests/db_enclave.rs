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
