use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

const EXPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct AccountExport {
    schema_version: u32,
    generated_at: String,
    profile: ProfileExport,
    notification_preferences: NotificationPrefs,
    sessions: Vec<SessionExport>,
    push_subscriptions: Vec<PushSubExport>,
    blocked_users: Vec<BlockedUserExport>,
    blocked_by: Vec<BlockedUserExport>,
    invite_codes_created: Vec<InviteCodeExport>,
    enclave_memberships: Vec<EnclaveMembership>,
    enclave_invitations_received: Vec<EnclaveInvitation>,
    room_memberships: Vec<RoomMembership>,
    room_notification_settings: Vec<RoomNotifSetting>,
    messages: Vec<MessageExport>,
    reactions_given: Vec<ReactionExport>,
    bookmarks: Vec<BookmarkExport>,
    pinned_messages: Vec<PinnedExport>,
    mentions_received: Vec<MentionExport>,
    file_uploads: Vec<FileUploadExport>,
    custom_emojis_uploaded: Vec<CustomEmojiExport>,
    dm_read_state: Vec<DmReadState>,
    moderation_actions_against_me: Vec<ModActionExport>,
}

#[derive(Serialize)]
struct ProfileExport {
    user_id: String,
    username: String,
    display_name: Option<String>,
    role: String,
    bio: Option<String>,
    avatar_extension: Option<String>,
    status: String,
    custom_status: Option<String>,
    email: Option<String>,
    email_verified_at: Option<String>,
    is_profile_public: bool,
    totp_enabled: bool,
    created_at: String,
    updated_at: String,
    last_active_at: String,
    is_banned: bool,
    ban_reason: Option<String>,
    banned_until: Option<String>,
    is_muted: bool,
    mute_reason: Option<String>,
    muted_until: Option<String>,
}

#[derive(Serialize)]
struct NotificationPrefs {
    read_receipts_enabled: bool,
    notify_browser_enabled: bool,
    notify_sound_enabled: bool,
    notify_push_enabled: bool,
    notify_email_digest_enabled: bool,
}

#[derive(Serialize)]
struct SessionExport {
    id: String,
    created_at: String,
    expires_at: String,
    last_seen_at: Option<String>,
    user_agent: Option<String>,
    ip: Option<String>,
}

#[derive(Serialize)]
struct PushSubExport {
    id: i64,
    endpoint: String,
    user_agent: Option<String>,
    created_at: String,
    last_seen_at: String,
}

#[derive(Serialize)]
struct BlockedUserExport {
    user_id: String,
    username: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct InviteCodeExport {
    id: i64,
    code: String,
    used_by: Option<String>,
    used_at: Option<String>,
    expires_at: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct EnclaveMembership {
    enclave_id: i64,
    enclave_name: String,
    role: String,
    joined_at: String,
}

#[derive(Serialize)]
struct EnclaveInvitation {
    id: i64,
    enclave_id: i64,
    enclave_name: String,
    invited_by: String,
    created_at: String,
}

#[derive(Serialize)]
struct RoomMembership {
    room_id: i64,
    room_name: String,
    room_type: String,
    enclave_id: Option<i64>,
    joined_at: String,
}

#[derive(Serialize)]
struct RoomNotifSetting {
    room_id: i64,
    mute_mode: String,
    muted_until: Option<String>,
    updated_at: String,
}

#[derive(Serialize)]
struct MessageExport {
    id: i64,
    room_id: i64,
    parent_id: Option<i64>,
    body: String,
    created_at: String,
    edited_at: Option<String>,
    deleted_at: Option<String>,
}

#[derive(Serialize)]
struct ReactionExport {
    message_id: i64,
    emoji: String,
    created_at: String,
}

#[derive(Serialize)]
struct BookmarkExport {
    message_id: i64,
    created_at: String,
}

#[derive(Serialize)]
struct PinnedExport {
    message_id: i64,
    room_id: i64,
    pinned_at: String,
}

#[derive(Serialize)]
struct MentionExport {
    id: i64,
    message_id: i64,
    room_id: i64,
    author_user_id: String,
    created_at: String,
    read_at: Option<String>,
}

#[derive(Serialize)]
struct FileUploadExport {
    id: i64,
    message_id: Option<i64>,
    filename: String,
    mime_type: String,
    size_bytes: i64,
    download_url: String,
    created_at: String,
}

#[derive(Serialize)]
struct CustomEmojiExport {
    id: i64,
    enclave_id: i64,
    shortcode: String,
    mime_type: String,
    size_bytes: i64,
    created_at: String,
}

#[derive(Serialize)]
struct DmReadState {
    room_id: i64,
    last_read_message_id: i64,
    updated_at: String,
}

#[derive(Serialize)]
struct ModActionExport {
    id: i64,
    action: String,
    actor_user: String,
    reason: Option<String>,
    room_id: Option<i64>,
    metadata: Option<String>,
    created_at: String,
}

/// `GET /settings/export-data` - download the signed-in user's full account
/// data as a JSON file. Covers profile, notification prefs, sessions, push
/// subs, blocks, enclave/room memberships, messages, reactions, bookmarks,
/// pins, mentions, file uploads (metadata + URL), custom emojis, DM read
/// state, and moderation actions taken against the user. File attachments
/// and avatar bytes are not bundled; their URLs are referenced so the same
/// authenticated user can fetch them via the existing `/api/files/{id}`
/// and `/avatars/{user_id}` endpoints.
pub async fn get_export(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let data = collect_export(&state.auth, &state.chat, &user.id).await?;
    let body = serde_json::to_vec_pretty(&data)
        .map_err(|e| AppError::Internal(format!("export json: {e}")))?;

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let filename = format!(
        "lets-chat-export-{}-{}.json",
        sanitize_filename_component(&user.username),
        stamp
    );
    let disposition = format!("attachment; filename=\"{filename}\"");

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_string(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

async fn collect_export(
    auth: &SqlitePool,
    chat: &SqlitePool,
    user_id: &str,
) -> Result<AccountExport, AppError> {
    let profile = load_profile(auth, user_id).await?;
    let notification_preferences = load_prefs(auth, user_id).await?;
    let sessions = load_sessions(auth, user_id).await?;
    let push_subscriptions = load_push_subs(auth, user_id).await?;
    let blocked_users = load_blocks_outgoing(auth, user_id).await?;
    let blocked_by = load_blocks_incoming(auth, user_id).await?;
    let invite_codes_created = load_invite_codes(auth, user_id).await?;
    let enclave_memberships = load_enclave_memberships(chat, user_id).await?;
    let enclave_invitations_received = load_enclave_invitations(chat, user_id).await?;
    let room_memberships = load_room_memberships(chat, user_id).await?;
    let room_notification_settings = load_room_notif_settings(chat, user_id).await?;
    let messages = load_messages(chat, user_id).await?;
    let reactions_given = load_reactions(chat, user_id).await?;
    let bookmarks = load_bookmarks(chat, user_id).await?;
    let pinned_messages = load_pinned(chat, user_id).await?;
    let mentions_received = load_mentions(chat, user_id).await?;
    let file_uploads = load_file_uploads(chat, user_id).await?;
    let custom_emojis_uploaded = load_custom_emojis(chat, user_id).await?;
    let dm_read_state = load_dm_read_state(chat, user_id).await?;
    let moderation_actions_against_me = load_mod_actions(chat, user_id).await?;

    Ok(AccountExport {
        schema_version: EXPORT_SCHEMA_VERSION,
        generated_at: chrono::Utc::now().to_rfc3339(),
        profile,
        notification_preferences,
        sessions,
        push_subscriptions,
        blocked_users,
        blocked_by,
        invite_codes_created,
        enclave_memberships,
        enclave_invitations_received,
        room_memberships,
        room_notification_settings,
        messages,
        reactions_given,
        bookmarks,
        pinned_messages,
        mentions_received,
        file_uploads,
        custom_emojis_uploaded,
        dm_read_state,
        moderation_actions_against_me,
    })
}

async fn load_profile(pool: &SqlitePool, user_id: &str) -> Result<ProfileExport, AppError> {
    let row = sqlx::query(
        "SELECT id, username, display_name, role, bio, avatar_ext, status, custom_status, \
                email, email_verified_at, is_profile_public, totp_enabled, \
                created_at, updated_at, last_active_at, \
                is_banned, ban_reason, banned_until, \
                is_muted, mute_reason, muted_until \
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(ProfileExport {
        user_id: row.get("id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        bio: row.get("bio"),
        avatar_extension: row.get("avatar_ext"),
        status: row.get("status"),
        custom_status: row.get("custom_status"),
        email: row.get("email"),
        email_verified_at: row.get("email_verified_at"),
        is_profile_public: row.get::<i64, _>("is_profile_public") != 0,
        totp_enabled: row.get::<i64, _>("totp_enabled") != 0,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_active_at: row.get("last_active_at"),
        is_banned: row.get::<i64, _>("is_banned") != 0,
        ban_reason: row.get("ban_reason"),
        banned_until: row.get("banned_until"),
        is_muted: row.get::<i64, _>("is_muted") != 0,
        mute_reason: row.get("mute_reason"),
        muted_until: row.get("muted_until"),
    })
}

async fn load_prefs(pool: &SqlitePool, user_id: &str) -> Result<NotificationPrefs, AppError> {
    let row = sqlx::query(
        "SELECT read_receipts_enabled, notify_browser_enabled, notify_sound_enabled, \
                notify_push_enabled, notify_email_digest_enabled \
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    Ok(NotificationPrefs {
        read_receipts_enabled: row.get::<i64, _>("read_receipts_enabled") != 0,
        notify_browser_enabled: row.get::<i64, _>("notify_browser_enabled") != 0,
        notify_sound_enabled: row.get::<i64, _>("notify_sound_enabled") != 0,
        notify_push_enabled: row.get::<i64, _>("notify_push_enabled") != 0,
        notify_email_digest_enabled: row.get::<i64, _>("notify_email_digest_enabled") != 0,
    })
}

async fn load_sessions(pool: &SqlitePool, user_id: &str) -> Result<Vec<SessionExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, created_at, expires_at, last_seen_at, user_agent, ip \
         FROM sessions WHERE user_id = ? ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionExport {
            id: r.get("id"),
            created_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
            last_seen_at: r.get("last_seen_at"),
            user_agent: r.get("user_agent"),
            ip: r.get("ip"),
        })
        .collect())
}

async fn load_push_subs(pool: &SqlitePool, user_id: &str) -> Result<Vec<PushSubExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, endpoint, user_agent, created_at, last_seen_at \
         FROM push_subscriptions WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PushSubExport {
            id: r.get("id"),
            endpoint: r.get("endpoint"),
            user_agent: r.get("user_agent"),
            created_at: r.get("created_at"),
            last_seen_at: r.get("last_seen_at"),
        })
        .collect())
}

async fn load_blocks_outgoing(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<BlockedUserExport>, AppError> {
    let rows = sqlx::query(
        "SELECT b.blocked_id AS user_id, u.username AS username, b.created_at AS created_at \
         FROM user_blocks b LEFT JOIN users u ON u.id = b.blocked_id \
         WHERE b.blocker_id = ? ORDER BY b.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BlockedUserExport {
            user_id: r.get("user_id"),
            username: r.get("username"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_blocks_incoming(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<BlockedUserExport>, AppError> {
    let rows = sqlx::query(
        "SELECT b.blocker_id AS user_id, u.username AS username, b.created_at AS created_at \
         FROM user_blocks b LEFT JOIN users u ON u.id = b.blocker_id \
         WHERE b.blocked_id = ? ORDER BY b.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BlockedUserExport {
            user_id: r.get("user_id"),
            username: r.get("username"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_invite_codes(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<InviteCodeExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, code, used_by, used_at, expires_at, created_at \
         FROM invite_codes WHERE created_by = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| InviteCodeExport {
            id: r.get("id"),
            code: r.get("code"),
            used_by: r.get("used_by"),
            used_at: r.get("used_at"),
            expires_at: r.get("expires_at"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_enclave_memberships(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<EnclaveMembership>, AppError> {
    let rows = sqlx::query(
        "SELECT em.enclave_id, e.name AS enclave_name, em.role, em.joined_at \
         FROM enclave_members em JOIN enclaves e ON e.id = em.enclave_id \
         WHERE em.user_id = ? ORDER BY em.joined_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EnclaveMembership {
            enclave_id: r.get("enclave_id"),
            enclave_name: r.get("enclave_name"),
            role: r.get("role"),
            joined_at: r.get("joined_at"),
        })
        .collect())
}

async fn load_enclave_invitations(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<EnclaveInvitation>, AppError> {
    let rows = sqlx::query(
        "SELECT ei.id, ei.enclave_id, e.name AS enclave_name, ei.invited_by, ei.created_at \
         FROM enclave_invitations ei JOIN enclaves e ON e.id = ei.enclave_id \
         WHERE ei.invitee_id = ? ORDER BY ei.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EnclaveInvitation {
            id: r.get("id"),
            enclave_id: r.get("enclave_id"),
            enclave_name: r.get("enclave_name"),
            invited_by: r.get("invited_by"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_room_memberships(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<RoomMembership>, AppError> {
    let rows = sqlx::query(
        "SELECT rm.room_id, r.name AS room_name, r.room_type, r.enclave_id, rm.joined_at \
         FROM room_members rm JOIN rooms r ON r.id = rm.room_id \
         WHERE rm.user_id = ? ORDER BY rm.joined_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RoomMembership {
            room_id: r.get("room_id"),
            room_name: r.get("room_name"),
            room_type: r.get("room_type"),
            enclave_id: r.get("enclave_id"),
            joined_at: r.get("joined_at"),
        })
        .collect())
}

async fn load_room_notif_settings(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<RoomNotifSetting>, AppError> {
    let rows = sqlx::query(
        "SELECT room_id, mute_mode, muted_until, updated_at \
         FROM room_notification_settings WHERE user_id = ? ORDER BY room_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RoomNotifSetting {
            room_id: r.get("room_id"),
            mute_mode: r.get("mute_mode"),
            muted_until: r.get("muted_until"),
            updated_at: r.get("updated_at"),
        })
        .collect())
}

async fn load_messages(pool: &SqlitePool, user_id: &str) -> Result<Vec<MessageExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, room_id, parent_id, body, created_at, edited_at, deleted_at \
         FROM messages WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MessageExport {
            id: r.get("id"),
            room_id: r.get("room_id"),
            parent_id: r.get("parent_id"),
            body: r.get("body"),
            created_at: r.get("created_at"),
            edited_at: r.get("edited_at"),
            deleted_at: r.get("deleted_at"),
        })
        .collect())
}

async fn load_reactions(pool: &SqlitePool, user_id: &str) -> Result<Vec<ReactionExport>, AppError> {
    let rows = sqlx::query(
        "SELECT message_id, emoji, created_at \
         FROM message_reactions WHERE user_id = ? ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ReactionExport {
            message_id: r.get("message_id"),
            emoji: r.get("emoji"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_bookmarks(pool: &SqlitePool, user_id: &str) -> Result<Vec<BookmarkExport>, AppError> {
    let rows = sqlx::query(
        "SELECT message_id, created_at FROM bookmarks WHERE user_id = ? ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BookmarkExport {
            message_id: r.get("message_id"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_pinned(pool: &SqlitePool, user_id: &str) -> Result<Vec<PinnedExport>, AppError> {
    let rows = sqlx::query(
        "SELECT message_id, room_id, pinned_at FROM pinned_messages \
         WHERE pinned_by = ? ORDER BY pinned_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PinnedExport {
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            pinned_at: r.get("pinned_at"),
        })
        .collect())
}

async fn load_mentions(pool: &SqlitePool, user_id: &str) -> Result<Vec<MentionExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, message_id, room_id, author_user_id, created_at, read_at \
         FROM mentions WHERE mentioned_user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MentionExport {
            id: r.get("id"),
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            author_user_id: r.get("author_user_id"),
            created_at: r.get("created_at"),
            read_at: r.get("read_at"),
        })
        .collect())
}

async fn load_file_uploads(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<FileUploadExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, message_id, filename, mime_type, size_bytes, created_at \
         FROM file_uploads WHERE uploader_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let id: i64 = r.get("id");
            FileUploadExport {
                id,
                message_id: r.get("message_id"),
                filename: r.get("filename"),
                mime_type: r.get("mime_type"),
                size_bytes: r.get("size_bytes"),
                download_url: format!("/api/files/{id}"),
                created_at: r.get("created_at"),
            }
        })
        .collect())
}

async fn load_custom_emojis(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<CustomEmojiExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, enclave_id, shortcode, mime_type, size_bytes, created_at \
         FROM custom_emojis WHERE uploaded_by = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CustomEmojiExport {
            id: r.get("id"),
            enclave_id: r.get("enclave_id"),
            shortcode: r.get("shortcode"),
            mime_type: r.get("mime_type"),
            size_bytes: r.get("size_bytes"),
            created_at: r.get("created_at"),
        })
        .collect())
}

async fn load_dm_read_state(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<DmReadState>, AppError> {
    let rows = sqlx::query(
        "SELECT room_id, last_read_message_id, updated_at \
         FROM dm_read_state WHERE user_id = ? ORDER BY room_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DmReadState {
            room_id: r.get("room_id"),
            last_read_message_id: r.get("last_read_message_id"),
            updated_at: r.get("updated_at"),
        })
        .collect())
}

async fn load_mod_actions(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<ModActionExport>, AppError> {
    let rows = sqlx::query(
        "SELECT id, action, actor_user, reason, room_id, metadata, created_at \
         FROM mod_actions WHERE target_user = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ModActionExport {
            id: r.get("id"),
            action: r.get("action"),
            actor_user: r.get("actor_user"),
            reason: r.get("reason"),
            room_id: r.get("room_id"),
            metadata: r.get("metadata"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Restrict the filename component to safe characters for use inside the
/// `Content-Disposition` header. Usernames are constrained by the auth
/// rules already, but the header is still user-controlled bytes so we
/// pin it to ASCII alphanumerics, dash, and underscore and fall back to
/// "user" if the result is empty.
fn sanitize_filename_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}
