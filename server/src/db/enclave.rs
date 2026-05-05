use sqlx::{Row, SqlitePool};

use crate::models::enclave::{Enclave, EnclaveInvitation, EnclaveMembership, EnclaveRole};

pub async fn get_general_id(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("id")))
}

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

/// Idempotent. No-op when General has any members. Reads users from auth pool,
/// writes membership rows + General.created_by into chat pool.
pub async fn backfill_general_membership(
    auth: &SqlitePool,
    chat: &SqlitePool,
) -> Result<(), sqlx::Error> {
    let Some(general_row) = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_optional(chat)
        .await?
    else {
        return Ok(());
    };
    let general_id: i64 = general_row.get("id");

    let any_member = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? LIMIT 1")
        .bind(general_id)
        .fetch_optional(chat)
        .await?;
    if any_member.is_some() {
        return Ok(());
    }

    let users = sqlx::query("SELECT id, role FROM users ORDER BY created_at ASC, id ASC")
        .fetch_all(auth)
        .await?;
    let any_admin = users.iter().any(|u| u.get::<String, _>("role") == "admin");
    if !any_admin {
        return Ok(());
    }
    let mut owner_id: Option<String> = None;
    for u in &users {
        let id: String = u.get("id");
        let role: String = u.get("role");
        let target_role = if role == "admin" {
            if owner_id.is_none() {
                owner_id = Some(id.clone());
                "owner"
            } else {
                "admin"
            }
        } else {
            "member"
        };
        sqlx::query(
            "INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, ?)",
        )
        .bind(general_id)
        .bind(&id)
        .bind(target_role)
        .execute(chat)
        .await?;
    }
    if let Some(o) = owner_id {
        sqlx::query("UPDATE enclaves SET created_by=? WHERE id=? AND created_by='system'")
            .bind(&o)
            .bind(general_id)
            .execute(chat)
            .await?;
    }
    Ok(())
}

pub async fn update_metadata(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    description: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET name=?, description=? WHERE id=?")
        .bind(name)
        .bind(description)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_public(pool: &SqlitePool, id: i64, is_public: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET is_public=? WHERE id=?")
        .bind(if is_public { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn regenerate_invite_code(
    pool: &SqlitePool,
    id: i64,
    new_code: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET invite_code=? WHERE id=?")
        .bind(new_code)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_invite_code(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET invite_code=NULL WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_enclave(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclaves WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_invitation(
    pool: &SqlitePool,
    enclave_id: i64,
    invitee_id: &str,
    invited_by: &str,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO enclave_invitations (enclave_id, invitee_id, invited_by) VALUES (?, ?, ?)",
    )
    .bind(enclave_id)
    .bind(invitee_id)
    .bind(invited_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn list_invitations_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(EnclaveInvitation, Enclave)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.id, i.enclave_id, i.invitee_id, i.invited_by, i.created_at, \
                e.id AS e_id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at AS e_created_at \
         FROM enclave_invitations i \
         JOIN enclaves e ON e.id = i.enclave_id \
         WHERE i.invitee_id = ? \
         ORDER BY i.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let inv = EnclaveInvitation {
                id: r.get("id"),
                enclave_id: r.get("enclave_id"),
                invitee_id: r.get("invitee_id"),
                invited_by: r.get("invited_by"),
                created_at: r.get("created_at"),
            };
            let enc = Enclave {
                id: r.get("e_id"),
                name: r.get("name"),
                description: r.get("description"),
                is_public: r.get::<i64, _>("is_public") != 0,
                invite_code: r.get("invite_code"),
                created_by: r.get("created_by"),
                created_at: r.get("e_created_at"),
            };
            (inv, enc)
        })
        .collect())
}

pub async fn get_invitation(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<EnclaveInvitation>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, enclave_id, invitee_id, invited_by, created_at FROM enclave_invitations WHERE id=?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| EnclaveInvitation {
        id: r.get("id"),
        enclave_id: r.get("enclave_id"),
        invitee_id: r.get("invitee_id"),
        invited_by: r.get("invited_by"),
        created_at: r.get("created_at"),
    }))
}

pub async fn delete_invitation(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclave_invitations WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn accept_invitation(pool: &SqlitePool, id: i64) -> Result<(i64, String), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT enclave_id, invitee_id FROM enclave_invitations WHERE id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let row = row.ok_or(sqlx::Error::RowNotFound)?;
    let enclave_id: i64 = row.get("enclave_id");
    let invitee_id: String = row.get("invitee_id");
    sqlx::query(
        "INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, 'member')",
    )
    .bind(enclave_id)
    .bind(&invitee_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM enclave_invitations WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((enclave_id, invitee_id))
}

pub async fn transfer_ownership(
    pool: &SqlitePool,
    enclave_id: i64,
    new_owner_id: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let exists = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(new_owner_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    sqlx::query("UPDATE enclave_members SET role='admin' WHERE enclave_id=? AND role='owner'")
        .bind(enclave_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE enclave_members SET role='owner' WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(new_owner_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
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
