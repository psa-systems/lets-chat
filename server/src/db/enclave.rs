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

pub async fn get_enclave(pool: &SqlitePool, id: i64) -> Result<Option<Enclave>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE id=?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_enclave))
}

pub async fn get_enclave_by_invite_code(
    pool: &SqlitePool,
    code: &str,
) -> Result<Option<Enclave>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE invite_code = ?",
    )
    .bind(code)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_enclave))
}

pub async fn add_member(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    role: EnclaveRole,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, ?)",
    )
    .bind(enclave_id)
    .bind(user_id)
    .bind(role.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_member(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_role(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    role: EnclaveRole,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclave_members SET role=? WHERE enclave_id=? AND user_id=?")
        .bind(role.as_str())
        .bind(enclave_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_enclaves_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<Enclave>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at \
         FROM enclaves e \
         JOIN enclave_members m ON m.enclave_id = e.id AND m.user_id = ? \
         ORDER BY e.name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_enclave).collect())
}

pub async fn list_public_enclaves(pool: &SqlitePool) -> Result<Vec<Enclave>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE is_public = 1 ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_enclave).collect())
}

pub async fn list_members(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<EnclaveMembership>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT enclave_id, user_id, role, joined_at FROM enclave_members WHERE enclave_id = ? ORDER BY joined_at",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let role_str: String = r.get("role");
        let role = EnclaveRole::from_str(&role_str).map_err(|e| sqlx::Error::Decode(e.into()))?;
        out.push(EnclaveMembership {
            enclave_id: r.get("enclave_id"),
            user_id: r.get("user_id"),
            role,
            joined_at: r.get("joined_at"),
        });
    }
    Ok(out)
}

pub async fn get_membership(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<Option<EnclaveMembership>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT enclave_id, user_id, role, joined_at \
         FROM enclave_members WHERE enclave_id=? AND user_id=?",
    )
    .bind(enclave_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let role_str: String = r.get("role");
    let role = EnclaveRole::from_str(&role_str).map_err(|e| sqlx::Error::Decode(e.into()))?;
    Ok(Some(EnclaveMembership {
        enclave_id: r.get("enclave_id"),
        user_id: r.get("user_id"),
        role,
        joined_at: r.get("joined_at"),
    }))
}
