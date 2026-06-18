use sqlx::{Row, SqlitePool};
use std::str::FromStr;

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
        share_emojis_globally: row.get::<i64, _>("share_emojis_globally") != 0,
        msg_rate_limit_burst: row.get::<i64, _>("msg_rate_limit_burst").max(0) as u32,
        coyote_mode: row.get::<i64, _>("coyote_mode") != 0,
        shame_tags_enabled: row.get::<i64, _>("shame_tags_enabled") != 0,
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
        "SELECT id, name, description, is_public, invite_code, created_by, created_at, share_emojis_globally, msg_rate_limit_burst, coyote_mode, shame_tags_enabled \
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
        "SELECT id, name, description, is_public, invite_code, created_by, created_at, share_emojis_globally, msg_rate_limit_burst, coyote_mode, shame_tags_enabled \
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

/// LC-160: is `user_id` a member of `enclave_id`? Used to authorize an
/// `enclave:{id}` WebSocket topic subscription before the connection is added
/// to that topic's fan-out set.
pub async fn is_enclave_member(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
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

/// Idempotent. Inserts missing General memberships for every existing auth-DB
/// user when at least one site admin exists, and repairs the General enclave's
/// `created_by` field when it is still the `'system'` sentinel. Safe to call
/// multiple times: per-row INSERTs use `INSERT OR IGNORE`, and the
/// `created_by` UPDATE is gated on the sentinel value.
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

pub async fn set_share_emojis_globally(
    pool: &SqlitePool,
    id: i64,
    share: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET share_emojis_globally=? WHERE id=?")
        .bind(if share { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-217: set the per-enclave message send rate-limit burst (per minute).
/// `0` clears the override and falls back to the global cap.
pub async fn set_msg_rate_limit_burst(
    pool: &SqlitePool,
    id: i64,
    burst: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET msg_rate_limit_burst=? WHERE id=?")
        .bind(burst as i64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-342: toggle the "shame tag" community-moderation prototype for an enclave.
pub async fn set_shame_tags_enabled(
    pool: &SqlitePool,
    id: i64,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET shame_tags_enabled=? WHERE id=?")
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-342: is shame-tagging enabled for the enclave that owns `room_id`?
/// `false` for DMs, missing rooms, or enclaves with the prototype off.
pub async fn shame_tags_enabled_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COALESCE(e.shame_tags_enabled, 0) AS st \
         FROM rooms r LEFT JOIN enclaves e ON e.id = r.enclave_id WHERE r.id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<i64, _>("st") != 0).unwrap_or(false))
}

/// LC-339: toggle "Coyote Mode" anti-spam for an enclave.
pub async fn set_coyote_mode(pool: &SqlitePool, id: i64, enabled: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET coyote_mode=? WHERE id=?")
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-339: cheap helper for the burst-detection gate. Resolves the room's
/// enclave and its `coyote_mode` flag in one JOIN. Returns `None` for a DM
/// (`rooms.enclave_id IS NULL`) or a missing row; `Some((enclave_id, false))`
/// when the room is in an enclave with the mode off.
pub async fn get_coyote_mode_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<Option<(i64, bool)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT r.enclave_id AS eid, COALESCE(e.coyote_mode, 0) AS cm \
         FROM rooms r LEFT JOIN enclaves e ON e.id = r.enclave_id \
         WHERE r.id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| {
        r.get::<Option<i64>, _>("eid")
            .map(|eid| (eid, r.get::<i64, _>("cm") != 0))
    }))
}

/// LC-339: count how many DISTINCT rooms of `enclave_id` the user has posted a
/// (non-deleted) message in within the last `secs` seconds. The just-inserted
/// message counts. `>= 3` over a 3 s window is the bot signal.
pub async fn count_distinct_rooms_posted_recently(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    secs: i64,
) -> Result<i64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT m.room_id) FROM messages m \
         JOIN rooms r ON r.id = m.room_id \
         WHERE m.user_id = ? AND r.enclave_id = ? AND m.deleted_at IS NULL \
           AND m.created_at >= datetime('now', '-' || ? || ' seconds')",
    )
    .bind(user_id)
    .bind(enclave_id)
    .bind(secs)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// LC-339: ban a user from an enclave (kick + ban-list so they cannot rejoin).
/// Idempotent on the ban row; always removes membership.
pub async fn ban_from_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT OR IGNORE INTO enclave_bans (enclave_id, user_id, reason) VALUES (?, ?, ?)",
    )
    .bind(enclave_id)
    .bind(user_id)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// LC-339: is this user on the enclave's ban-list?
pub async fn is_enclave_banned(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM enclave_bans WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// LC-340: a row of the enclave ban-list, newest first. `user_id` is raw; the
/// caller resolves a display label.
pub struct EnclaveBanRow {
    pub user_id: String,
    pub reason: Option<String>,
    pub banned_at: String,
}

/// LC-340: list an enclave's bans for the settings UI, newest first.
pub async fn list_enclave_bans(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<EnclaveBanRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT user_id, reason, banned_at FROM enclave_bans \
         WHERE enclave_id=? ORDER BY banned_at DESC, user_id ASC",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EnclaveBanRow {
            user_id: r.get("user_id"),
            reason: r.get("reason"),
            banned_at: r.get("banned_at"),
        })
        .collect())
}

/// LC-340: lift an enclave ban. After this the user may rejoin and post again.
pub async fn unban_from_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclave_bans WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-217: cheap helper for `post_message`'s rate-limit gate. JOINs
/// `rooms` -> `enclaves` so the caller does not need a separate
/// "what enclave is this room in" query. Returns `0` when the room is a
/// DM (`rooms.enclave_id IS NULL`), the row is missing, or the column is
/// 0; all three mean "use the global cap, no per-enclave override".
/// Also returns the enclave id (for the rate-limit composite key) so the
/// caller can scope the counter properly. `None` for the id when the
/// room has no enclave.
pub async fn get_msg_rate_limit_burst_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<(Option<i64>, u32), sqlx::Error> {
    let row = sqlx::query(
        "SELECT r.enclave_id AS eid, COALESCE(e.msg_rate_limit_burst, 0) AS burst \
         FROM rooms r \
         LEFT JOIN enclaves e ON e.id = r.enclave_id \
         WHERE r.id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|r| {
            let eid: Option<i64> = r.get("eid");
            let burst = r.get::<i64, _>("burst").max(0) as u32;
            (eid, burst)
        })
        .unwrap_or((None, 0)))
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

/// Return the set of user ids that currently have an outstanding invitation
/// to `enclave_id`. Used by the invite typeahead to render "Invited" instead
/// of the Invite button without a per-row lookup.
pub async fn pending_invitee_ids_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT invitee_id FROM enclave_invitations WHERE enclave_id = ?")
        .bind(enclave_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.get("invitee_id")).collect())
}

pub async fn list_invitations_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(EnclaveInvitation, Enclave)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.id, i.enclave_id, i.invitee_id, i.invited_by, i.created_at, \
                e.id AS e_id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at AS e_created_at, e.share_emojis_globally, e.msg_rate_limit_burst, e.coyote_mode, e.shame_tags_enabled \
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
                share_emojis_globally: r.get::<i64, _>("share_emojis_globally") != 0,
                msg_rate_limit_burst: r.get::<i64, _>("msg_rate_limit_burst").max(0) as u32,
                coyote_mode: r.get::<i64, _>("coyote_mode") != 0,
                shame_tags_enabled: r.get::<i64, _>("shame_tags_enabled") != 0,
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
        "SELECT e.id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at, e.share_emojis_globally, e.msg_rate_limit_burst, e.coyote_mode, e.shame_tags_enabled \
         FROM enclaves e \
         JOIN enclave_members m ON m.enclave_id = e.id AND m.user_id = ? \
         ORDER BY e.name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(map_enclave).collect())
}

pub async fn list_all_enclaves_with_counts(
    pool: &SqlitePool,
) -> Result<Vec<(Enclave, i64, Option<String>)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at, e.share_emojis_globally, e.msg_rate_limit_burst, e.coyote_mode, e.shame_tags_enabled, \
                (SELECT COUNT(*) FROM enclave_members m WHERE m.enclave_id = e.id) AS member_count, \
                (SELECT user_id FROM enclave_members m WHERE m.enclave_id = e.id AND m.role = 'owner' LIMIT 1) AS owner_id \
         FROM enclaves e ORDER BY e.name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let enclave = Enclave {
                id: r.get("id"),
                name: r.get("name"),
                description: r.get("description"),
                is_public: r.get::<i64, _>("is_public") != 0,
                invite_code: r.get("invite_code"),
                created_by: r.get("created_by"),
                created_at: r.get("created_at"),
                share_emojis_globally: r.get::<i64, _>("share_emojis_globally") != 0,
                msg_rate_limit_burst: r.get::<i64, _>("msg_rate_limit_burst").max(0) as u32,
                coyote_mode: r.get::<i64, _>("coyote_mode") != 0,
                shame_tags_enabled: r.get::<i64, _>("shame_tags_enabled") != 0,
            };
            let count: i64 = r.get("member_count");
            let owner: Option<String> = r.get("owner_id");
            (enclave, count, owner)
        })
        .collect())
}

pub async fn list_public_enclaves(pool: &SqlitePool) -> Result<Vec<Enclave>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at, share_emojis_globally, msg_rate_limit_burst, coyote_mode, shame_tags_enabled \
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

/// True when `a` and `b` share at least one enclave. Used to gate
/// username-based block lookups so a caller cannot probe for the existence
/// of arbitrary private accounts.
pub async fn users_share_enclave(pool: &SqlitePool, a: &str, b: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT 1 FROM enclave_members m1 \
         JOIN enclave_members m2 ON m2.enclave_id = m1.enclave_id \
         WHERE m1.user_id = ? AND m2.user_id = ? LIMIT 1",
    )
    .bind(a)
    .bind(b)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
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

/// LC-143: record the room a user last opened in an enclave (upsert).
pub async fn set_last_room(
    pool: &SqlitePool,
    user_id: &str,
    enclave_id: i64,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO enclave_last_room (user_id, enclave_id, room_id, updated_at) \
         VALUES (?, ?, ?, datetime('now')) \
         ON CONFLICT(user_id, enclave_id) DO UPDATE SET \
            room_id = excluded.room_id, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(enclave_id)
    .bind(room_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// LC-143: the room a user last opened in an enclave, if any. The caller
/// validates it is still accessible before redirecting.
pub async fn get_last_room(
    pool: &SqlitePool,
    user_id: &str,
    enclave_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row =
        sqlx::query("SELECT room_id FROM enclave_last_room WHERE user_id = ? AND enclave_id = ?")
            .bind(user_id)
            .bind(enclave_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.get::<i64, _>("room_id")))
}
