use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

use crate::auth::{AuthUser, SESSION_COOKIE};
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::ws::events::ChatEvent;

const CONFIRM_PHRASE: &str = "delete my account";

#[derive(Deserialize)]
pub struct DeleteAccountForm {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub confirm_phrase: String,
}

/// `POST /settings/delete-account` - permanently delete the signed-in user.
///
/// Self-serve account deletion. The caller must:
/// 1. Type the literal phrase `delete my account` so a stray click on the
///    button cannot wipe the account.
/// 2. On standalone, re-enter their password so a stolen session cookie
///    alone cannot complete the deletion. The saas build skips this step
///    because authentication is delegated to the upstream IdP.
///
/// The handler refuses if the caller is the sole `owner` of an enclave
/// that still has other members - they must transfer ownership or delete
/// the enclave first, otherwise that enclave would be left with no owner
/// row and the unique partial index would prevent another owner from
/// being installed without manual intervention.
///
/// On success the user row is removed (auth.db FKs cascade sessions,
/// blocks, password-reset tokens, email-verification tokens and
/// pending_2fa rows), all chat.db rows belonging to or referencing the
/// user are purged, the avatar file is removed from disk, the session
/// cookie is cleared, and `ChatEvent::UserBanned` is broadcast so any
/// other connected clients drop the user from their UI.
pub async fn post_delete_account(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    axum::Form(form): axum::Form<DeleteAccountForm>,
) -> Result<Response, AppError> {
    if form.confirm_phrase.trim().to_ascii_lowercase() != CONFIRM_PHRASE {
        return Err(AppError::BadRequest(format!(
            "Type \"{CONFIRM_PHRASE}\" to confirm."
        )));
    }

    #[cfg(feature = "standalone")]
    {
        let password = form.password.as_deref().unwrap_or("");
        if password.is_empty() {
            return Err(AppError::BadRequest(
                "Enter your current password to confirm.".to_string(),
            ));
        }
        let record = db::auth::find_user_by_id(&state.auth, &user.id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if !verify_password(&record.password_hash, password) {
            return Err(AppError::BadRequest("Password is incorrect.".to_string()));
        }
    }
    #[cfg(not(feature = "standalone"))]
    {
        let _ = form.password;
    }

    if would_orphan_admin_role(&state.auth, &user).await? {
        return Err(AppError::BadRequest(
            "You are the only admin. Promote another user to admin before deleting your account."
                .to_string(),
        ));
    }

    let blockers = sole_owner_enclaves(&state.chat, &user.id).await?;
    if !blockers.is_empty() {
        let names = blockers
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::BadRequest(format!(
            "Transfer ownership or delete these enclaves first: {names}."
        )));
    }

    purge_user(&state, &user.id).await?;

    state.hub.broadcast_global(&ChatEvent::UserBanned {
        user_id: user.id.clone(),
    });

    let mut clear = Cookie::new(SESSION_COOKIE, "");
    clear.set_path("/");
    clear.make_removal();
    let jar = jar.remove(clear);
    Ok((jar, Redirect::to("/login")).into_response())
}

/// True when the caller is the only remaining `admin` and another user
/// exists. The first-user auto-promote only fires when the users table
/// drops to a single row, so deleting the sole admin while non-admin
/// accounts still exist would lock the instance out of admin recovery
/// short of a manual SQL fix. Refuse the delete and tell the caller to
/// promote someone else first.
async fn would_orphan_admin_role(auth: &SqlitePool, user: &User) -> Result<bool, AppError> {
    if user.role != "admin" {
        return Ok(false);
    }
    let other_admins: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin' AND id <> ?")
            .bind(&user.id)
            .fetch_one(auth)
            .await?;
    if other_admins > 0 {
        return Ok(false);
    }
    let other_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id <> ?")
        .bind(&user.id)
        .fetch_one(auth)
        .await?;
    Ok(other_users > 0)
}

/// Return the names of enclaves where the user is the sole `owner` AND at
/// least one other member remains. An enclave the user owns alone is fine
/// to remove during account deletion; one with other members is not,
/// because the unique partial index on `enclave_members(enclave_id) WHERE
/// role='owner'` would block any other member from being promoted later.
async fn sole_owner_enclaves(chat: &SqlitePool, user_id: &str) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        "SELECT e.name AS name \
         FROM enclave_members em \
         JOIN enclaves e ON e.id = em.enclave_id \
         WHERE em.user_id = ? AND em.role = 'owner' \
           AND (SELECT COUNT(*) FROM enclave_members em2 WHERE em2.enclave_id = em.enclave_id) > 1",
    )
    .bind(user_id)
    .fetch_all(chat)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("name"))
        .collect())
}

/// Wipe every row and file belonging to or referencing `user_id`.
///
/// Order matters: in `auth.db`, `invite_codes.created_by` has a FK with no
/// `ON DELETE` clause, so the row must be cleared before the parent user
/// row is removed. Push subscriptions have no FK at all and must also be
/// removed explicitly. Everything else in `auth.db` cascades from the
/// `users` row.
///
/// `chat.db` lives in a separate SQLite file, so there is no foreign-key
/// path from `auth.users` to anything in chat. We delete user-attributed
/// rows directly. Deleting the user's `messages` rows cascades to that
/// message's reactions, mentions, pins, bookmarks, and thread replies via
/// `parent_id ON DELETE CASCADE`; the explicit row-level deletes below
/// catch the user's footprint on messages authored by others (their own
/// reactions, bookmarks, mentions, pins, etc.).
async fn purge_user(state: &AppState, user_id: &str) -> Result<(), AppError> {
    // Avatar on disk: best-effort, don't fail the request if the file is
    // already missing. The DB row goes away when we delete the user.
    if let Ok(Some(record)) = db::auth::find_user_by_id(&state.auth, user_id).await {
        if let Some(ext) = record.avatar_ext.as_deref() {
            let path = db::avatars_dir().join(format!("{}.{ext}", record.id));
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    purge_user_chat(&state.chat, user_id).await?;
    purge_user_auth(&state.auth, user_id).await?;

    state.activity_ledger.remove(user_id);
    Ok(())
}

async fn purge_user_chat(chat: &SqlitePool, user_id: &str) -> Result<(), AppError> {
    let mut tx = chat.begin().await?;
    // The user's own messages. The CASCADE rules attached to messages
    // remove their reactions, mentions, pins, bookmarks, and any thread
    // replies whose parent_id pointed at them. File-upload rows have
    // `message_id ON DELETE SET NULL` so they survive and become
    // orphans, but we drop them next anyway.
    sqlx::query("DELETE FROM messages WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // The user's footprint on messages authored by others.
    sqlx::query("DELETE FROM message_reactions WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM bookmarks WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pinned_messages WHERE pinned_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE mentioned_user_id = ? OR author_user_id = ?")
        .bind(user_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // LC-62: drop scheduled rows authored by this user. The dispatcher's
    // "author no longer exists" branch in validate_delivery covers any
    // window between auth-row delete and chat-cleanup, but the contract
    // is that by the time this transaction commits there are no rows
    // left for the dispatcher to encounter.
    db::scheduled::delete_for_user(&mut tx, user_id).await?;
    sqlx::query("DELETE FROM file_uploads WHERE uploader_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE uploaded_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM room_members WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM room_notification_settings WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM dm_read_state WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // Drop solo-owned enclaves entirely (members cascade off `enclaves`),
    // then the remaining enclave_members row covers cases where the user
    // was a non-owner member.
    sqlx::query(
        "DELETE FROM enclaves WHERE id IN ( \
            SELECT em.enclave_id FROM enclave_members em \
            WHERE em.user_id = ? AND em.role = 'owner' \
              AND (SELECT COUNT(*) FROM enclave_members em2 \
                   WHERE em2.enclave_id = em.enclave_id) = 1 \
         )",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM enclave_members WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM enclave_invitations WHERE invitee_id = ? OR invited_by = ?")
        .bind(user_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // Drop moderation history that targeted the deleted user. Rows where
    // the user was the actor stay so the audit trail against other
    // accounts remains intact.
    sqlx::query("DELETE FROM mod_actions WHERE target_user = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn purge_user_auth(auth: &SqlitePool, user_id: &str) -> Result<(), AppError> {
    let mut tx = auth.begin().await?;
    // invite_codes has FK to users(id) with no ON DELETE, so we have to
    // unbind any rows the user still appears on before the user row goes
    // away. Codes the user created cannot stand on their own (created_by
    // is NOT NULL) so they're deleted outright; codes the user merely
    // redeemed have their used_by cleared so the audit row stays.
    sqlx::query("DELETE FROM invite_codes WHERE created_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE invite_codes SET used_by = NULL, used_at = NULL WHERE used_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // No FK; remove manually.
    sqlx::query("DELETE FROM push_subscriptions WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // The remaining auth-side tables (sessions, user_blocks,
    // password_reset_tokens, email_verification_tokens, pending_2fa) all
    // declare ON DELETE CASCADE against users, so this final statement
    // sweeps them with it.
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "standalone")]
fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
