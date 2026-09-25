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
    pub confirm_phrase: String,
}

/// `POST /settings/delete-account` - permanently delete the signed-in user.
///
/// LC-22 cutover: the password re-confirmation step is gone (no password
/// exists). The caller must still type the literal phrase `delete my account`
/// so a stray click cannot wipe the account. If an operator wants stronger
/// reconfirmation, that work is upstream at Bunyip's session policy.
///
/// The handler refuses if the caller is the sole `owner` of an enclave
/// that still has other members - they must transfer ownership or delete
/// the enclave first.
///
/// On success the user row is removed (auth.db FKs cascade sessions /
/// blocks / etc.), all chat.db rows belonging to or referencing the user
/// are purged, the avatar file is removed from disk, the session cookie
/// is cleared, and `ChatEvent::UserBanned` is broadcast so any other
/// connected clients drop the user from their UI.
pub async fn post_delete_account(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    jar: CookieJar,
    axum::Form(form): axum::Form<DeleteAccountForm>,
) -> Result<Response, AppError> {
    // LC-356: a failed delete-account submission flashes inline on /settings
    // (the form lives there) instead of throwing a full-page error.
    if form.confirm_phrase.trim().to_ascii_lowercase() != CONFIRM_PHRASE {
        return Ok(crate::routes::settings::settings_error_redirect(&format!(
            "Type \"{CONFIRM_PHRASE}\" to confirm account deletion."
        )));
    }

    if would_orphan_admin_role(&state.auth, &user).await? {
        return Ok(crate::routes::settings::settings_error_redirect(
            "You are the only admin. Promote another user to admin before deleting your account.",
        ));
    }

    let blockers = sole_owner_enclaves(&state.chat, &user.id).await?;
    if !blockers.is_empty() {
        let names = blockers
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(crate::routes::settings::settings_error_redirect(&format!(
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
    // LC-63: drop reminders this user set (on any message). Reminders on the
    // user's own messages also cascade when those messages are deleted, but
    // this clears reminders set on other people's messages too.
    sqlx::query("DELETE FROM reminders WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // LC-64: drop server-persisted drafts. message_drafts.room_id has
    // ON DELETE CASCADE so room deletion alone covers room-vanishing
    // cleanup; this clears the user's drafts in rooms that survive
    // the purge (every room they were drafting in).
    db::drafts::delete_for_user(&mut *tx, user_id).await?;
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
    // LC-923: a ban record for the deleted user and any unexpired
    // email-reply token must not outlive the account.
    sqlx::query("DELETE FROM enclave_bans WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reply_tokens WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // LC-1017: support_tickets and remote_control_events were added after
    // LC-908/LC-923 and missed both the purge and the schema-walking guard's
    // suffix list.
    sqlx::query("DELETE FROM support_tickets WHERE requester_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM remote_control_events WHERE actor_id = ? OR target_id = ?")
        .bind(user_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // LC-908: the remaining user-scoped tables the original enumeration
    // missed. See `chat_user_columns_are_purged_or_allowlisted` in
    // `tests/routes_account_delete.rs` for the schema-walking guard that
    // keeps this list complete as new tables are added.
    sqlx::query("DELETE FROM poll_votes WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM saved_searches WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM thread_followers WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM thread_muters WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM kudos WHERE giver_id = ? OR receiver_id = ?")
        .bind(user_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM message_acks WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM canned_responses WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM room_role_overrides WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // A grant issued by this user against a room they no longer belong to
    // (or never belonged to) stays as a row; only the issuer reference is
    // cleared, matching the mod_actions actor-preservation convention but
    // without leaving a dangling user id an admin UI could render.
    sqlx::query("UPDATE room_role_overrides SET assigned_by = '' WHERE assigned_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM room_nicknames WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM user_group_members WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM message_reports WHERE reporter_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE message_reports SET handled_by = NULL WHERE handled_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // Deleting the followups row cascades to its followup_items (FK on
    // followup_items.message_id ON DELETE CASCADE); this only covers lists
    // the user created. Items they self-claimed or checked off in someone
    // else's list are cleared separately below.
    sqlx::query("DELETE FROM followups WHERE created_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE followup_items SET assignee_id = NULL WHERE assignee_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE followup_items SET done_by = NULL WHERE done_by = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM user_storage_quotas WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM message_tag_overrides WHERE actor_user = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM enclave_last_room WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // LC-947: dm_pairs.user_lo/user_hi exist only to enforce the
    // find-or-create uniqueness constraint (LC-909); once the deleted user
    // can no longer be a DM participant, the row has no functional purpose
    // and would otherwise retain their id indefinitely. room_members above
    // already dropped this user's membership, so the room is orphaned on
    // their side regardless; this just stops dm_pairs from being the one
    // place their id survives.
    sqlx::query("DELETE FROM dm_pairs WHERE user_lo = ? OR user_hi = ?")
        .bind(user_id)
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
