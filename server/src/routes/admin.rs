use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

use crate::auth::AdminUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::admin::{
    AdminEnclaveView, AdminInviteView, AdminRoomView, AdminUserView, EnclavesPage, InvitesPage,
    ModLogPage, RoomRowFragment, RoomsPage, SettingsPage, UserRowFragment, UsersPage,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin", get(get_settings))
        .route("/admin/settings", get(get_settings).post(post_settings))
        .route(
            "/admin/settings/email-digest-default",
            post(post_email_digest_default),
        )
        .route("/admin/users", get(get_users))
        .route("/admin/users/{id}/ban", post(post_ban))
        .route("/admin/users/{id}/unban", post(post_unban))
        .route("/admin/users/{id}/mute", post(post_mute))
        .route("/admin/users/{id}/unmute", post(post_unmute))
        .route("/admin/users/{id}/role", post(post_role))
        .route("/admin/users/{id}/delete", post(post_delete_user))
        .route("/admin/users/{id}/sso/unlink", post(post_unlink_sso))
        .route("/admin/invites", get(get_invites).post(post_create_invite))
        .route("/admin/invites/{id}/revoke", post(post_revoke_invite))
        .route("/admin/rooms", get(get_rooms))
        .route("/admin/rooms/{id}/archive", post(post_archive_room))
        .route("/admin/rooms/{id}/edit", post(post_edit_room))
        .route("/admin/rooms/{id}/invite", post(post_invite_to_room))
        .route("/admin/rooms/{id}/regenerate", post(post_regenerate_invite))
        .route("/admin/enclaves", get(get_enclaves))
        .route("/admin/modlog", get(get_modlog))
        .route(
            "/admin/uploads/regenerate-thumbnails",
            post(post_regenerate_thumbnails),
        )
        .route("/admin/uploads/purge-orphans", post(post_purge_orphans))
}

pub async fn get_enclaves(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let raw = db::enclave::list_all_enclaves_with_counts(&state.chat).await?;
    let enclaves: Vec<AdminEnclaveView> = raw
        .into_iter()
        .map(|(e, count, owner_id)| AdminEnclaveView {
            id: e.id,
            name: e.name,
            description: e.description,
            is_public: e.is_public,
            invite_code: e.invite_code,
            member_count: count,
            owner_id,
            created_at: e.created_at,
        })
        .collect();
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let page = EnclavesPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "enclaves",
        enclaves: &enclaves,
    };
    html(&page)
}

// Settings ------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SettingsForm {
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_user: String,
    pub smtp_from: String,
    pub smtp_pass: String,
}

#[derive(Deserialize, Default)]
pub struct SettingsQuery {
    pub regenerated: Option<i64>,
    pub purged: Option<i64>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(q): Query<SettingsQuery>,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let smtp_host = db::settings::get_setting(&state.settings, "smtp_host")
        .await?
        .unwrap_or_default();
    let smtp_port = db::settings::get_setting(&state.settings, "smtp_port")
        .await?
        .unwrap_or_else(|| "587".to_string());
    let smtp_user = db::settings::get_setting(&state.settings, "smtp_user")
        .await?
        .unwrap_or_default();
    let smtp_from = db::settings::get_setting(&state.settings, "smtp_from")
        .await?
        .unwrap_or_default();
    let default_notify_email_digest =
        db::settings::get_setting(&state.settings, "default_notify_email_digest")
            .await?
            .as_deref()
            == Some("1");
    let uploads_total_bytes = db::uploads::sum_size_bytes(&state.chat).await?;
    let uploads_total_display =
        format!("{:.2} MiB", uploads_total_bytes as f64 / (1024.0 * 1024.0));
    let uploads_orphan_count = db::uploads::count_orphans(&state.chat).await?;
    let page = SettingsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "settings",
        smtp_host,
        smtp_port,
        smtp_user,
        smtp_from,
        default_notify_email_digest,
        saved: false,
        uploads_total_display,
        uploads_orphan_count,
        regenerated: q.regenerated,
        purged: q.purged,
    };
    html(&page)
}

/// Walk every image upload and write a preview for any row that lacks one on
/// disk. Useful when upgrading from a pre-Phase-23 deployment, or to recover
/// from a batch of failed preview writes (disk-full transients, etc.).
/// Originals are not touched: pre-Phase-23 originals were not stripped at
/// upload time and would lose their content-addressed identity if we
/// re-encoded them now, so this action is preview-only.
pub async fn post_regenerate_thumbnails(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> Result<Redirect, AppError> {
    let rows = db::uploads::list_image_uploads(&state.chat).await?;
    let uploads_dir = db::uploads_dir();
    let mut regenerated: i64 = 0;
    for row in rows {
        let preview_name = crate::uploads::preview_storage_name(&row.storage_path);
        let preview_path = uploads_dir.join(&preview_name);
        if tokio::fs::metadata(&preview_path).await.is_ok() {
            continue;
        }
        let original_path = uploads_dir.join(&row.storage_path);

        let permit = crate::uploads::thumbnail_semaphore()
            .acquire()
            .await
            .expect("thumbnail semaphore never closed");
        let path_for_blocking = original_path.clone();
        let mime_for_blocking = row.mime_type.clone();
        let preview_result = tokio::task::spawn_blocking(move || {
            crate::uploads::pipeline::preview_from_path(&path_for_blocking, &mime_for_blocking)
        })
        .await
        .map_err(|e| AppError::Internal(format!("regen join: {e}")))?;
        drop(permit);

        match preview_result {
            Ok(bytes) => {
                if let Err(e) = crate::uploads::write_atomic(&preview_path, &bytes).await {
                    tracing::warn!(
                        error = %e,
                        row_id = row.id,
                        "regenerate: preview write failed",
                    );
                } else {
                    regenerated += 1;
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    row_id = row.id,
                    "regenerate: pipeline failed",
                );
            }
        }
    }
    tracing::info!(regenerated, "admin regenerate-thumbnails complete");
    Ok(Redirect::to(&format!(
        "/admin/settings?regenerated={regenerated}"
    )))
}

/// Run the orphan sweeper with threshold = 0, i.e. consider every
/// `message_id IS NULL` row a candidate regardless of age. Reuses
/// `run_orphan_sweep` so the same dedup-aware transaction and the same
/// missing-file-is-success semantics apply; the only difference vs the
/// hourly tick is the threshold.
pub async fn post_purge_orphans(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> Result<Redirect, AppError> {
    let stats = crate::uploads::sweep::run_orphan_sweep(&state.chat, 0)
        .await
        .map_err(|e| AppError::Internal(format!("purge orphans: {e}")))?;
    tracing::info!(
        rows = stats.rows_deleted,
        files = stats.files_deleted,
        errors = stats.errors,
        "admin purge-orphans complete",
    );
    Ok(Redirect::to(&format!(
        "/admin/settings?purged={}",
        stats.rows_deleted
    )))
}

pub async fn post_settings(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    axum::Form(form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    db::settings::set_setting(&state.settings, "smtp_host", &form.smtp_host).await?;
    db::settings::set_setting(&state.settings, "smtp_port", &form.smtp_port).await?;
    db::settings::set_setting(&state.settings, "smtp_user", &form.smtp_user).await?;
    db::settings::set_setting(&state.settings, "smtp_from", &form.smtp_from).await?;
    if !form.smtp_pass.is_empty() {
        db::settings::set_setting(&state.settings, "smtp_pass", &form.smtp_pass).await?;
    }
    Ok(Redirect::to("/admin/settings").into_response())
}

#[derive(Deserialize)]
pub struct EmailDigestDefaultForm {
    /// Form-checkbox convention: a checked box submits the field, an
    /// unchecked box omits it.
    #[serde(default)]
    pub default_notify_email_digest: Option<String>,
}

/// Flip the "new users start with digest enabled" instance default.
/// Existing users are NOT retroactively changed; only the registration
/// flow consults this key.
pub async fn post_email_digest_default(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    axum::Form(form): axum::Form<EmailDigestDefaultForm>,
) -> Result<Response, AppError> {
    let value = if form.default_notify_email_digest.is_some() {
        "1"
    } else {
        "0"
    };
    db::settings::set_setting(&state.settings, "default_notify_email_digest", value).await?;
    Ok(Redirect::to("/admin/settings").into_response())
}

// Users ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: String,
}

pub async fn get_users(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let records = db::auth::list_users(&state.auth).await?;
    let mut users: Vec<AdminUserView> = Vec::with_capacity(records.len());
    for r in records {
        let sso_identities = db::sso::list_sso_identities_for_user(&state.auth, &r.id)
            .await?
            .into_iter()
            .map(|s| crate::views::admin::AdminUserSsoIdentity {
                issuer: s.issuer,
                email: s.email,
            })
            .collect();
        let has_password = !r.password_hash.is_empty();
        users.push(AdminUserView {
            id: r.id,
            username: r.username,
            role: r.role,
            is_banned: r.is_banned,
            is_muted: r.is_muted,
            has_password,
            sso_identities,
        });
    }
    let page = UsersPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "users",
        users: &users,
    };
    html(&page)
}

pub async fn post_ban(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    db::auth::ban_user(&state.auth, &user_id, None).await?;
    db::moderation::log_mod_action(&state.chat, "ban", &user_id, &actor.id, None, None, None)
        .await?;
    state.hub.broadcast_global(&ChatEvent::UserBanned {
        user_id: user_id.clone(),
    });
    render_user_row(&state, &user_id).await
}

pub async fn post_unban(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    db::auth::unban_user(&state.auth, &user_id).await?;
    db::moderation::log_mod_action(&state.chat, "unban", &user_id, &actor.id, None, None, None)
        .await?;
    render_user_row(&state, &user_id).await
}

pub async fn post_mute(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    db::auth::mute_user(&state.auth, &user_id, None, None).await?;
    db::moderation::log_mod_action(&state.chat, "mute", &user_id, &actor.id, None, None, None)
        .await?;
    state.hub.broadcast_global(&ChatEvent::UserMuted {
        user_id: user_id.clone(),
        muted_until: None,
    });
    render_user_row(&state, &user_id).await
}

pub async fn post_unmute(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    db::auth::unmute_user(&state.auth, &user_id).await?;
    db::moderation::log_mod_action(&state.chat, "unmute", &user_id, &actor.id, None, None, None)
        .await?;
    render_user_row(&state, &user_id).await
}

pub async fn post_role(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
    axum::Form(form): axum::Form<RoleForm>,
) -> Result<Html, AppError> {
    let role = form.role.as_str();
    if !matches!(role, "user" | "moderator" | "admin") {
        return Err(AppError::BadRequest("invalid role".into()));
    }
    db::auth::set_user_role(&state.auth, &user_id, role).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "role_change",
        &user_id,
        &actor.id,
        Some(role),
        None,
        None,
    )
    .await?;
    render_user_row(&state, &user_id).await
}

pub async fn post_delete_user(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    if user_id == actor.id {
        return Err(AppError::BadRequest("cannot delete yourself".into()));
    }
    db::auth::delete_user_sessions(&state.auth, &user_id).await?;
    db::auth::delete_user(&state.auth, &user_id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "delete_user",
        &user_id,
        &actor.id,
        None,
        None,
        None,
    )
    .await?;
    state.hub.broadcast_global(&ChatEvent::UserBanned {
        user_id: user_id.clone(),
    });
    Ok(Html(String::new()))
}

async fn render_user_row(state: &AppState, user_id: &str) -> Result<Html, AppError> {
    let record = db::auth::find_user_by_id(&state.auth, user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let sso_identities = db::sso::list_sso_identities_for_user(&state.auth, &record.id)
        .await?
        .into_iter()
        .map(|s| crate::views::admin::AdminUserSsoIdentity {
            issuer: s.issuer,
            email: s.email,
        })
        .collect();
    let has_password = !record.password_hash.is_empty();
    let view = AdminUserView {
        id: record.id,
        username: record.username,
        role: record.role,
        is_banned: record.is_banned,
        is_muted: record.is_muted,
        has_password,
        sso_identities,
    };
    html(&UserRowFragment { u: &view })
}

pub async fn post_unlink_sso(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    let removed = db::sso::unlink_sso_identity(&state.auth, &user_id).await?;
    tracing::info!(
        target: "lets_chat.auth.sso",
        event = "sso_admin_unlinked",
        actor = %actor.id,
        user_id = %user_id,
        removed_rows = removed,
        "admin removed SSO link"
    );
    render_user_row(&state, &user_id).await
}

// Invites -------------------------------------------------------------------

pub async fn get_invites(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let invites = build_invite_views(&state).await?;
    let page = InvitesPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "invites",
        invites: &invites,
    };
    html(&page)
}

pub async fn post_create_invite(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
) -> Result<Response, AppError> {
    let code = random_code(8);
    db::auth::create_invite_code(&state.auth, &code, &actor.id).await?;
    Ok(Redirect::to("/admin/invites").into_response())
}

pub async fn post_revoke_invite(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    Path(invite_id): Path<i64>,
) -> Result<Html, AppError> {
    db::auth::revoke_invite_code(&state.auth, invite_id).await?;
    Ok(Html(String::new()))
}

async fn build_invite_views(state: &AppState) -> Result<Vec<AdminInviteView>, AppError> {
    let codes = db::auth::list_invite_codes(&state.auth).await?;
    let mut views = Vec::with_capacity(codes.len());
    for c in codes {
        let created_by_username = db::auth::find_user_by_id(&state.auth, &c.created_by)
            .await?
            .map(|r| r.username)
            .unwrap_or_else(|| "(unknown)".to_string());
        let used_by_username = match c.used_by.as_deref() {
            Some(uid) => db::auth::find_user_by_id(&state.auth, uid)
                .await?
                .map(|r| r.username),
            None => None,
        };
        views.push(AdminInviteView {
            id: c.id,
            code: c.code,
            created_by_username,
            used_by_username,
            created_at: c.created_at,
        });
    }
    Ok(views)
}

// Rooms ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EditRoomForm {
    pub name: String,
    #[serde(default)]
    pub topic: String,
}

#[derive(Deserialize)]
pub struct InviteToRoomForm {
    pub username: String,
}

pub async fn get_rooms(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let raw_rooms = db::chat::list_rooms(&state.chat, &user.id, true).await?;
    let mut rooms_admin = Vec::with_capacity(raw_rooms.len());
    for r in &raw_rooms {
        let members = db::chat::count_room_members(&state.chat, r.id).await?;
        rooms_admin.push(AdminRoomView {
            id: r.id,
            name: r.name.clone(),
            topic: r.topic.clone(),
            room_type: r.room_type.clone(),
            invite_code: r.invite_code.clone(),
            members,
            created_at: r.created_at.clone(),
        });
    }
    let page = RoomsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "rooms",
        rooms_admin: &rooms_admin,
    };
    html(&page)
}

pub async fn post_archive_room(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    db::chat::delete_room(&state.chat, room_id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "delete_room",
        "",
        &actor.id,
        None,
        Some(room_id),
        None,
    )
    .await?;
    Ok(Html(String::new()))
}

pub async fn post_edit_room(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<EditRoomForm>,
) -> Result<Html, AppError> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let topic = if form.topic.trim().is_empty() {
        None
    } else {
        Some(form.topic.trim())
    };
    db::chat::update_room(&state.chat, room_id, name, topic).await?;
    render_room_row(&state, room_id).await
}

pub async fn post_invite_to_room(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<InviteToRoomForm>,
) -> Result<Html, AppError> {
    let username = form.username.trim();
    let target = db::auth::find_user_by_username(&state.auth, username)
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;
    db::chat::add_room_member(&state.chat, room_id, &target.id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "room_invite",
        &target.id,
        &actor.id,
        None,
        Some(room_id),
        None,
    )
    .await?;
    state.hub.broadcast_to_user(
        &target.id,
        &ChatEvent::RoomMemberAdded {
            room_id,
            user_id: target.id.clone(),
        },
    );
    render_room_row(&state, room_id).await
}

pub async fn post_regenerate_invite(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    let new_code = random_code(10);
    db::chat::regenerate_invite_code(&state.chat, room_id, &new_code).await?;
    render_room_row(&state, room_id).await
}

async fn render_room_row(state: &AppState, room_id: i64) -> Result<Html, AppError> {
    let r = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let members = db::chat::count_room_members(&state.chat, room_id).await?;
    let view = AdminRoomView {
        id: r.id,
        name: r.name,
        topic: r.topic,
        room_type: r.room_type,
        invite_code: r.invite_code,
        members,
        created_at: r.created_at,
    };
    html(&RoomRowFragment { r: &view })
}

// Mod log -------------------------------------------------------------------

pub async fn get_modlog(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let entries = db::moderation::list_mod_actions(&state.chat).await?;
    let page = ModLogPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "modlog",
        entries: &entries,
    };
    html(&page)
}

fn random_code(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}
