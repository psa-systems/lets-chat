use sqlx::{Row, SqlitePool};

use crate::models::enclave::{Enclave, EnclaveInvitation, EnclaveMembership, EnclaveRole};

fn map_enclave(row: &sqlx::sqlite::SqliteRow) -> Enclave {
    Enclave {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        is_public: row.get::<i64, _>("is_public") != 0,
        invite_code: row.get("invite_code"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    }
}

pub async fn create_enclave(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    creator_id: &str,
) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("INSERT INTO enclaves (name, description, created_by) VALUES (?, ?, ?)")
        .bind(name)
        .bind(description)
        .bind(creator_id)
        .execute(&mut *tx)
        .await?;
    let id = res.last_insert_rowid();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, 'owner')")
        .bind(id)
        .bind(creator_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}
