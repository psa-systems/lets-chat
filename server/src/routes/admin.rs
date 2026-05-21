use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query, State};
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
    AdminEnclaveView, AdminInviteView, AdminRoomView, AdminUserView, AnalyticsPage, AntiSpamPage,
    BackupRestorePage, BotRowView, BotsPage, BrandingPage, BuiltinCommandRowView, DeliveryRowView,
    EnclavesPage, InvitesPage, LinkFilterPage, LinkFilterRuleView, MetricCard, ModLogPage,
    OutgoingWebhookDeliveriesPage, OutgoingWebhookRowView, OutgoingWebhooksPage,
    QuarantineEntryView, QuarantinePage, RoomRowFragment, RoomsPage, SettingsPage,
    SlashCommandRowView, SlashCommandsPage, UserRowFragment, UsersPage, OUTGOING_EVENTS,
};
use crate::views::charts;
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
        .route("/admin/maintenance", post(post_maintenance))
        .route("/admin/users", get(get_users))
        .route("/admin/users/{id}/ban", post(post_ban))
        .route("/admin/users/{id}/unban", post(post_unban))
        .route("/admin/users/{id}/mute", post(post_mute))
        .route("/admin/users/{id}/unmute", post(post_unmute))
        .route("/admin/users/{id}/role", post(post_role))
        .route("/admin/users/{id}/quota", post(post_user_quota))
        .route("/admin/users/{id}/delete", post(post_delete_user))
        .route("/admin/invites", get(get_invites).post(post_create_invite))
        .route("/admin/invites/{id}/revoke", post(post_revoke_invite))
        .route("/admin/rooms", get(get_rooms))
        .route("/admin/rooms/{id}/archive", post(post_archive_room))
        .route("/admin/rooms/{id}/edit", post(post_edit_room))
        .route("/admin/rooms/{id}/invite", post(post_invite_to_room))
        .route("/admin/rooms/{id}/regenerate", post(post_regenerate_invite))
        .route("/admin/enclaves", get(get_enclaves))
        .route("/admin/enclaves/{id}/quota", post(post_enclave_quota))
        .route("/admin/anti-spam", get(get_anti_spam).post(post_anti_spam))
        .route(
            "/admin/link-filter",
            get(get_link_filter).post(post_link_filter),
        )
        .route(
            "/admin/link-filter/{id}/delete",
            post(post_link_filter_delete),
        )
        .route("/admin/backup-restore", get(get_backup_restore))
        .route("/admin/backup", post(post_backup))
        .route(
            "/admin/restore",
            // 10 GiB cap. The route is admin-only so the threat model
            // is narrow (compromised admin creds, or a typo'd file
            // picker), but an unlimited cap would let either fill
            // the disk before validation runs.
            post(post_restore).layer(DefaultBodyLimit::max(10 * 1024 * 1024 * 1024)),
        )
        .route(
            "/admin/branding",
            get(get_branding)
                .post(post_branding)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route("/admin/quarantine", get(get_quarantine))
        .route(
            "/admin/quarantine/{id}/approve",
            post(post_quarantine_approve),
        )
        .route(
            "/admin/quarantine/{id}/reject",
            post(post_quarantine_reject),
        )
        .route("/admin/modlog", get(get_modlog))
        .route("/admin/bots", get(get_bots).post(post_bots))
        .route("/admin/bots/{id}/disable", post(post_bot_disable))
        .route(
            "/admin/outgoing-webhooks",
            get(get_outgoing_webhooks).post(post_outgoing_webhooks),
        )
        .route(
            "/admin/outgoing-webhooks/{id}/rotate",
            post(post_outgoing_rotate),
        )
        .route(
            "/admin/outgoing-webhooks/{id}/toggle",
            post(post_outgoing_toggle),
        )
        .route(
            "/admin/outgoing-webhooks/{id}/delete",
            post(post_outgoing_delete),
        )
        .route(
            "/admin/outgoing-webhooks/{id}/deliveries",
            get(get_outgoing_deliveries),
        )
        .route(
            "/admin/slash-commands",
            get(get_slash_commands).post(post_slash_commands),
        )
        .route(
            "/admin/slash-commands/{id}/delete",
            post(post_slash_commands_delete),
        )
        .route("/admin/analytics", get(get_analytics))
        .route("/admin/analytics/recompute", post(post_recompute_analytics))
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
    let mut enclaves: Vec<AdminEnclaveView> = Vec::with_capacity(raw.len());
    for (e, count, owner_id) in raw {
        let usage_bytes = db::quota::sum_enclave_usage(&state.chat, e.id).await?;
        let quota_bytes = db::quota::get_enclave_quota(&state.chat, e.id).await?;
        enclaves.push(AdminEnclaveView {
            id: e.id,
            name: e.name,
            description: e.description,
            is_public: e.is_public,
            invite_code: e.invite_code,
            member_count: count,
            owner_id,
            created_at: e.created_at,
            usage_display: format_bytes_mib(usage_bytes),
            quota_mib_value: bytes_to_mib_input(quota_bytes),
        });
    }
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let page = EnclavesPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
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
    let maintenance_enabled = db::settings::get_setting(&state.settings, "maintenance_mode")
        .await?
        .as_deref()
        == Some("true");
    let maintenance_message = db::settings::get_setting(&state.settings, "maintenance_message")
        .await?
        .unwrap_or_default();
    let uploads_total_bytes = db::uploads::sum_size_bytes(&state.chat).await?;
    let uploads_total_display =
        format!("{:.2} MiB", uploads_total_bytes as f64 / (1024.0 * 1024.0));
    let uploads_orphan_count = db::uploads::count_orphans(&state.chat).await?;
    let page = SettingsPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
        maintenance_enabled,
        maintenance_message,
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

#[derive(Deserialize)]
pub struct MaintenanceForm {
    /// Checkbox: present when on, omitted when off.
    #[serde(default)]
    pub enabled: Option<String>,
    /// Free-text shown on the 503 page when maintenance is on. Empty
    /// hides the message block; the page still renders the heading.
    #[serde(default)]
    pub message: String,
}

/// Flip the global maintenance-mode flag and persist the operator-facing
/// message together. Audited via the moderation log so the modlog page
/// surfaces who toggled it and when. The middleware reads the flag on
/// the next request, so non-admin traffic sees the 503 within milliseconds
/// of submitting this form.
pub async fn post_maintenance(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<MaintenanceForm>,
) -> Result<Response, AppError> {
    let on = form.enabled.is_some();
    let value = if on { "true" } else { "false" };
    db::settings::set_setting(&state.settings, "maintenance_mode", value).await?;
    db::settings::set_setting(&state.settings, "maintenance_message", form.message.trim()).await?;
    let action = if on {
        "maintenance_on"
    } else {
        "maintenance_off"
    };
    let metadata = (!form.message.trim().is_empty()).then(|| form.message.trim());
    db::moderation::log_mod_action(&state.chat, action, "", &actor.id, None, None, metadata)
        .await?;
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let records = db::auth::list_users(&state.auth).await?;
    let mut users: Vec<AdminUserView> = Vec::with_capacity(records.len());
    for r in records {
        let usage_bytes = db::quota::sum_user_usage(&state.chat, &r.id).await?;
        let quota_bytes = db::quota::get_user_quota(&state.chat, &r.id).await?;
        users.push(AdminUserView {
            id: r.id,
            username: r.username,
            role: r.role,
            is_banned: r.is_banned,
            is_muted: r.is_muted,
            usage_display: format_bytes_mib(usage_bytes),
            quota_mib_value: bytes_to_mib_input(quota_bytes),
        });
    }
    let page = UsersPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
    let usage_bytes = db::quota::sum_user_usage(&state.chat, &record.id).await?;
    let quota_bytes = db::quota::get_user_quota(&state.chat, &record.id).await?;
    let view = AdminUserView {
        id: record.id,
        username: record.username,
        role: record.role,
        is_banned: record.is_banned,
        is_muted: record.is_muted,
        usage_display: format_bytes_mib(usage_bytes),
        quota_mib_value: bytes_to_mib_input(quota_bytes),
    };
    html(&UserRowFragment { u: &view })
}

// Invites -------------------------------------------------------------------

pub async fn get_invites(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let invites = build_invite_views(&state).await?;
    let page = InvitesPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
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
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let entries = db::moderation::list_mod_actions(&state.chat).await?;
    let page = ModLogPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
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

// Quotas (LC-93) ------------------------------------------------------------

#[derive(Deserialize)]
pub struct QuotaForm {
    /// Whole MiB, or empty to clear the cap. The form input is a plain
    /// `<input type="number">`; empty submit means "unlimited".
    #[serde(default)]
    pub quota_mib: String,
}

/// Parse the admin form's MiB input into a byte count. An empty (or
/// whitespace-only) value returns `Ok(None)` for "unlimited"; a
/// non-negative integer returns `Ok(Some(mib * 1024 * 1024))`. Anything
/// else 400s.
fn parse_quota_mib(s: &str) -> Result<Option<i64>, AppError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let n: i64 = t.parse().map_err(|_| {
        AppError::BadRequest(
            "quota must be a non-negative whole number of MiB or empty for unlimited".into(),
        )
    })?;
    if n < 0 {
        return Err(AppError::BadRequest("quota must be non-negative".into()));
    }
    Ok(Some(n.saturating_mul(1024 * 1024)))
}

/// Inverse of `parse_quota_mib`: byte count back to the form-input
/// string. Truncates to whole MiB; an admin that sets a sub-MiB quota
/// out-of-band would see the field round down here, which is fine for
/// a UI that only accepts whole MiB anyway.
fn bytes_to_mib_input(q: Option<i64>) -> String {
    match q {
        Some(b) => (b / (1024 * 1024)).to_string(),
        None => String::new(),
    }
}

fn format_bytes_mib(bytes: i64) -> String {
    format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// Flip a user's storage quota. Returns the re-rendered admin row
/// so the HTMX form on `/admin/users` updates in place.
pub async fn post_user_quota(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
    axum::Form(form): axum::Form<QuotaForm>,
) -> Result<Html, AppError> {
    let quota_bytes = parse_quota_mib(&form.quota_mib)?;
    // 404 before we touch anything: `user_storage_quotas` has no FK to
    // auth.users (the two live in different databases), so a typo in
    // the path would otherwise leave an orphan quota row behind.
    if db::auth::find_user_by_id(&state.auth, &user_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    db::quota::set_user_quota(&state.chat, &user_id, quota_bytes).await?;
    let metadata = quota_bytes
        .map(|b| b.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    db::moderation::log_mod_action(
        &state.chat,
        "quota_set_user",
        &user_id,
        &actor.id,
        None,
        None,
        Some(&metadata),
    )
    .await?;
    render_user_row(&state, &user_id).await
}

/// Flip an enclave's storage quota. The admin enclaves page is a
/// plain table (no per-row HTMX fragment yet), so this redirects back
/// to the page rather than returning a partial.
pub async fn post_enclave_quota(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(enclave_id): Path<i64>,
    axum::Form(form): axum::Form<QuotaForm>,
) -> Result<Redirect, AppError> {
    let quota_bytes = parse_quota_mib(&form.quota_mib)?;
    // 404 before logging: `set_enclave_quota` is a plain UPDATE, so a
    // bogus id would just be a silent zero-row write but the audit
    // log would still record an action that had no effect.
    if db::enclave::get_enclave(&state.chat, enclave_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    db::quota::set_enclave_quota(&state.chat, enclave_id, quota_bytes).await?;
    let metadata = quota_bytes
        .map(|b| b.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    db::moderation::log_mod_action(
        &state.chat,
        "quota_set_enclave",
        "",
        &actor.id,
        None,
        Some(enclave_id),
        Some(&metadata),
    )
    .await?;
    Ok(Redirect::to("/admin/enclaves"))
}

// Anti-spam (LC-94) ---------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct AntiSpamQuery {
    pub saved: Option<i64>,
}

#[derive(Deserialize)]
pub struct AntiSpamForm {
    #[serde(default)]
    pub rate_limit_messages: Option<String>,
    #[serde(default)]
    pub rate_limit_registrations: Option<String>,
    #[serde(default)]
    pub rate_limit_password_resets: Option<String>,
    /// Checkboxes: present when on, omitted when off.
    #[serde(default)]
    pub link_filter_enabled: Option<String>,
    #[serde(default)]
    pub honeypot_enabled: Option<String>,
}

pub async fn get_anti_spam(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(q): Query<AntiSpamQuery>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let page = AntiSpamPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "anti_spam",
        rate_limit_messages: crate::rate_limit::read_u32_setting(
            &state.settings,
            "rate_limit_messages",
        )
        .await,
        rate_limit_registrations: crate::rate_limit::read_u32_setting(
            &state.settings,
            "rate_limit_registrations",
        )
        .await,
        rate_limit_password_resets: crate::rate_limit::read_u32_setting(
            &state.settings,
            "rate_limit_password_resets",
        )
        .await,
        link_filter_enabled: db::settings::get_setting(&state.settings, "link_filter_enabled")
            .await?
            .as_deref()
            == Some("true"),
        honeypot_enabled: db::settings::get_setting(&state.settings, "honeypot_enabled")
            .await?
            .as_deref()
            == Some("true"),
        saved: q.saved.is_some(),
    };
    html(&page)
}

/// Persist the anti-spam toggles + caps in one shot. Non-numeric caps
/// fall back to "0" (disabled) so a bad input cannot silently leave
/// the previous value in place. Audit entry records which actor
/// changed what; the metadata blob is a compact summary of the final
/// state so the mod log is greppable.
pub async fn post_anti_spam(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<AntiSpamForm>,
) -> Result<Redirect, AppError> {
    fn cap_or_zero(s: &Option<String>) -> String {
        s.as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .and_then(|t| t.parse::<u32>().ok())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "0".to_string())
    }
    let msg = cap_or_zero(&form.rate_limit_messages);
    let reg = cap_or_zero(&form.rate_limit_registrations);
    let pwr = cap_or_zero(&form.rate_limit_password_resets);
    let link = if form.link_filter_enabled.is_some() {
        "true"
    } else {
        "false"
    };
    let hp = if form.honeypot_enabled.is_some() {
        "true"
    } else {
        "false"
    };
    db::settings::set_setting(&state.settings, "rate_limit_messages", &msg).await?;
    db::settings::set_setting(&state.settings, "rate_limit_registrations", &reg).await?;
    db::settings::set_setting(&state.settings, "rate_limit_password_resets", &pwr).await?;
    db::settings::set_setting(&state.settings, "link_filter_enabled", link).await?;
    db::settings::set_setting(&state.settings, "honeypot_enabled", hp).await?;
    let summary = format!("msg={msg} reg={reg} pwr={pwr} link={link} hp={hp}");
    db::moderation::log_mod_action(
        &state.chat,
        "anti_spam_settings",
        "",
        &actor.id,
        None,
        None,
        Some(&summary),
    )
    .await?;
    Ok(Redirect::to("/admin/anti-spam?saved=1"))
}

// Link-filter rules (LC-94) -------------------------------------------------

#[derive(Deserialize)]
pub struct LinkFilterForm {
    pub pattern: String,
    pub action: String,
}

async fn render_link_filter_page(
    state: &AppState,
    user: &crate::models::User,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let raw = db::anti_spam::list_rules(&state.chat).await?;
    let rules: Vec<LinkFilterRuleView> = raw
        .into_iter()
        .map(|r| LinkFilterRuleView {
            id: r.id,
            pattern: r.pattern,
            action: r.action.as_str().to_string(),
            created_by: r.created_by,
            created_at: r.created_at,
        })
        .collect();
    let page = LinkFilterPage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "link_filter",
        rules: &rules,
        error,
    };
    html(&page)
}

pub async fn get_link_filter(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_link_filter_page(&state, &user, None).await
}

pub async fn post_link_filter(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<LinkFilterForm>,
) -> Result<Response, AppError> {
    let pattern = form.pattern.trim().to_ascii_lowercase();
    if pattern.is_empty() {
        return Ok(
            render_link_filter_page(&state, &actor, Some("Pattern is required".into()))
                .await?
                .into_response(),
        );
    }
    let Some(action) = db::anti_spam::FilterAction::parse(form.action.as_str()) else {
        return Ok(render_link_filter_page(
            &state,
            &actor,
            Some("Action must be block, quarantine, or warn".into()),
        )
        .await?
        .into_response());
    };
    match db::anti_spam::insert_rule(&state.chat, &pattern, action, &actor.id).await {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return Ok(render_link_filter_page(
                &state,
                &actor,
                Some("That pattern is already in the list".into()),
            )
            .await?
            .into_response());
        }
        Err(e) => return Err(AppError::from(e)),
    }
    db::moderation::log_mod_action(
        &state.chat,
        "link_filter_add",
        "",
        &actor.id,
        Some(&pattern),
        None,
        Some(action.as_str()),
    )
    .await?;
    Ok(Redirect::to("/admin/link-filter").into_response())
}

pub async fn post_link_filter_delete(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(rule_id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::anti_spam::delete_rule(&state.chat, rule_id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "link_filter_remove",
        "",
        &actor.id,
        None,
        None,
        Some(&rule_id.to_string()),
    )
    .await?;
    Ok(Redirect::to("/admin/link-filter"))
}

// Quarantine review (LC-94) -------------------------------------------------

pub async fn get_quarantine(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let raw = db::anti_spam::list_pending_quarantine(&state.chat).await?;
    let entries: Vec<QuarantineEntryView> = raw
        .into_iter()
        .map(|q| QuarantineEntryView {
            message_id: q.message_id,
            room_id: q.room_id,
            author_id: q.author_id,
            body: q.body,
            matched_pattern: q.matched_pattern,
            matched_url: q.matched_url,
            created_at: q.created_at,
        })
        .collect();
    let page = QuarantinePage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "quarantine",
        entries: &entries,
    };
    html(&page)
}

pub async fn post_quarantine_approve(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(message_id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::anti_spam::approve_quarantine(&state.chat, message_id, &actor.id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "quarantine_approve",
        "",
        &actor.id,
        None,
        None,
        Some(&message_id.to_string()),
    )
    .await?;
    // LC-94 follow-up: broadcast the freshly-unhidden message so
    // anyone currently in the room sees it appear live, not just on
    // the next page load. Mirrors the WS fanout from
    // `routes::room::post_message`. If the message or room has been
    // hard-deleted between hold + approve we just skip the
    // broadcast - the approve-decision is still logged.
    if let Ok(Some(raw)) = db::chat::get_message(&state.chat, message_id).await {
        if let Ok(Some(room)) = db::chat::get_room(&state.chat, raw.room_id).await {
            let author_name = db::auth::find_user_by_id(&state.auth, &raw.user_id)
                .await?
                .map(|r| r.username)
                .unwrap_or_else(|| "(unknown)".to_string());
            let message = crate::models::Message {
                id: raw.id,
                room_id: raw.room_id,
                user_id: raw.user_id,
                author_name,
                body: raw.body,
                created_at: raw.created_at,
                edited_at: raw.edited_at,
                parent_id: raw.parent_id,
                quote_id: raw.quote_id,
                is_system: raw.is_system,
                webhook_id: raw.webhook_id,
            };
            let event = ChatEvent::NewMessage {
                message,
                is_dm: room.room_type == "dm",
            };
            let _ = super::broadcast_room_message(&state, &room, &event).await;
        }
    }
    Ok(Redirect::to("/admin/quarantine"))
}

pub async fn post_quarantine_reject(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(message_id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::anti_spam::reject_quarantine(&state.chat, message_id, &actor.id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "quarantine_reject",
        "",
        &actor.id,
        None,
        None,
        Some(&message_id.to_string()),
    )
    .await?;
    Ok(Redirect::to("/admin/quarantine"))
}

// Backup / restore (LC-95) ------------------------------------------------

async fn render_backup_page(
    state: &AppState,
    user: &crate::models::User,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let data_dir = std::path::PathBuf::from(db::data_dir());
    let restore_pending = crate::backup::marker_path_for(&data_dir).exists();
    let page = BackupRestorePage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "backup",
        restore_pending,
        error,
    };
    html(&page)
}

pub async fn get_backup_restore(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_backup_page(&state, &user, None).await
}

/// Build the archive to a tempfile, stream it back, and unlink the
/// tempfile from disk while still holding the read handle. Linux
/// keeps the bytes accessible until the last close, so the file is
/// reclaimable the moment the response finishes (or aborts). The
/// orphan-on-disk window between create and unlink is one syscall.
pub async fn post_backup(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
) -> Result<Response, AppError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let download_name = format!("lets-chat-backup-{ts}.zip");
    let tmp_path = std::env::temp_dir().join(format!("lc-backup-{}.zip", uuid::Uuid::new_v4()));
    let data_dir = std::path::PathBuf::from(db::data_dir());
    let manifest = crate::backup::build_archive(
        &state.auth,
        &state.chat,
        &state.settings,
        &data_dir,
        &tmp_path,
    )
    .await?;
    let file = tokio::fs::File::open(&tmp_path)
        .await
        .map_err(|e| AppError::Internal(format!("reopen archive: {e}")))?;
    let size = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| AppError::Internal(format!("stat archive: {e}")))?;
    // Audit only after we have a sized file ready to stream. The
    // metadata payload deliberately leaves the system tempfile path
    // out - it serves no admin-debug purpose and just clutters the
    // log.
    db::moderation::log_mod_action(
        &state.chat,
        "backup_create",
        "",
        &actor.id,
        None,
        None,
        Some(&format!("{} files, {} bytes", manifest.files.len(), size)),
    )
    .await?;
    // Unlink immediately: the open handle keeps the bytes available
    // until the last close, so the file is released the moment the
    // stream finishes or the client disconnects.
    let _ = std::fs::remove_file(&tmp_path);
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    let disposition = format!("attachment; filename=\"{download_name}\"");
    Ok((
        axum::http::StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_string(),
            ),
            (axum::http::header::CONTENT_DISPOSITION, disposition),
            (axum::http::header::CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response())
}

pub async fn post_restore(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let tmp_path = std::env::temp_dir().join(format!("lc-restore-{}.zip", uuid::Uuid::new_v4()));
    let mut wrote_any = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() != Some("archive") {
            continue;
        }
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| AppError::Internal(format!("create tmp: {e}")))?;
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = field.next().await {
            let chunk = chunk.map_err(|e| AppError::BadRequest(format!("multipart read: {e}")))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| AppError::Internal(format!("write tmp: {e}")))?;
        }
        file.flush()
            .await
            .map_err(|e| AppError::Internal(format!("flush tmp: {e}")))?;
        wrote_any = true;
        break;
    }
    if !wrote_any {
        return Err(AppError::BadRequest("missing `archive` field".into()));
    }

    // Validate before staging so a tampered archive never lands on
    // disk in a location the next startup would consume. Both
    // `verify_archive` and `stage_extract` are synchronous filesystem
    // work; defer to the blocking pool so the tokio worker stays
    // free for a multi-GB archive.
    let tmp_for_verify = tmp_path.clone();
    let manifest =
        match tokio::task::spawn_blocking(move || crate::backup::verify_archive(&tmp_for_verify))
            .await
            .map_err(|e| AppError::Internal(format!("verify join: {e}")))?
        {
            Ok(m) => m,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                let msg = match &e {
                    AppError::BadRequest(s) => s.clone(),
                    _ => "archive validation failed".to_string(),
                };
                return Ok(render_backup_page(&state, &actor, Some(msg))
                    .await?
                    .into_response());
            }
        };

    let data_dir = std::path::PathBuf::from(db::data_dir());
    let staged = crate::backup::staged_dir_for(&data_dir);
    let tmp_for_stage = tmp_path.clone();
    let staged_for_stage = staged.clone();
    let stage_res = tokio::task::spawn_blocking(move || {
        crate::backup::stage_extract(&tmp_for_stage, &staged_for_stage)
    })
    .await
    .map_err(|e| AppError::Internal(format!("stage join: {e}")))?;
    if let Err(e) = stage_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    let _ = std::fs::remove_file(&tmp_path);

    // Drop the marker last so the page-refresh-banner check is
    // accurate from the next request onward.
    let marker = crate::backup::marker_path_for(&data_dir);
    std::fs::write(&marker, b"")
        .map_err(|e| AppError::Internal(format!("write restore marker: {e}")))?;

    db::moderation::log_mod_action(
        &state.chat,
        "restore_stage",
        "",
        &actor.id,
        None,
        None,
        Some(&format!(
            "archive version {} files {}",
            manifest.version,
            manifest.files.len()
        )),
    )
    .await?;
    Ok(Redirect::to("/admin/backup-restore").into_response())
}

// Branding (LC-96) ---------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct BrandingQuery {
    pub saved: Option<i64>,
}

async fn render_branding_page(
    state: &AppState,
    user: &crate::models::User,
    saved: bool,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let branding = db::branding::resolve(&state.chat, db::branding::Scope::Global).await?;
    let page = BrandingPage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "branding",
        primary_color: branding.primary_color,
        accent_color: branding.accent_color,
        login_heading: branding.login_heading,
        login_body: branding.login_body,
        has_logo: branding.logo_upload_id.is_some(),
        has_favicon: branding.favicon_upload_id.is_some(),
        saved,
        error,
    };
    html(&page)
}

pub async fn get_branding(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(q): Query<BrandingQuery>,
) -> Result<Html, AppError> {
    render_branding_page(&state, &user, q.saved.is_some(), None).await
}

/// Multipart branding upsert. Delegates the multipart parsing +
/// logo-file persistence to `branding::parse_branding_multipart` so
/// the per-enclave handler can reuse the exact same logic against a
/// different scope.
pub async fn post_branding(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let form = match super::branding::parse_branding_multipart(&state, &actor.id, multipart).await?
    {
        Ok(f) => f,
        Err(msg) => {
            return Ok(render_branding_page(&state, &actor, false, Some(msg))
                .await?
                .into_response());
        }
    };
    let existing = db::branding::resolve(&state.chat, db::branding::Scope::Global).await?;
    let logo_upload_id = form.new_logo_id.or(existing.logo_upload_id);
    let favicon_upload_id = form.new_favicon_id.or(existing.favicon_upload_id);
    let primary = form.primary_color.unwrap_or(existing.primary_color);
    let accent = form.accent_color.unwrap_or(existing.accent_color);
    if !db::branding::is_valid_hex_color(&primary) || !db::branding::is_valid_hex_color(&accent) {
        return Ok(render_branding_page(
            &state,
            &actor,
            false,
            Some("Colors must be #rgb or #rrggbb hex".into()),
        )
        .await?
        .into_response());
    }
    let heading = form.login_heading.unwrap_or(existing.login_heading);
    let body = form.login_body.unwrap_or(existing.login_body);

    db::branding::upsert(
        &state.chat,
        db::branding::Scope::Global,
        logo_upload_id,
        favicon_upload_id,
        &primary,
        &accent,
        &heading,
        &body,
        &actor.id,
    )
    .await?;
    db::moderation::log_mod_action(
        &state.chat,
        "branding_set",
        "",
        &actor.id,
        None,
        None,
        Some("global"),
    )
    .await?;
    Ok(Redirect::to("/admin/branding?saved=1").into_response())
}

/// Query string for the analytics dashboard. `days` selects the
/// look-back window; anything outside the supported set falls back to 30.
#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub days: Option<i64>,
}

/// Form body for the "Recompute today" button.
#[derive(Deserialize)]
pub struct RecomputeForm {
    pub days: Option<i64>,
}

/// Clamp an arbitrary `days` query value to a supported range button.
fn normalize_range(days: Option<i64>) -> i64 {
    match days {
        Some(7) => 7,
        Some(90) => 90,
        _ => 30,
    }
}

/// LC-97: admin analytics dashboard. Reads pre-aggregated daily metrics
/// from `analytics_daily` (fast, indexed) and computes the retention
/// triangle on demand, then renders each metric as an inline-SVG chart.
pub async fn get_analytics(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Html, AppError> {
    let range_days = normalize_range(q.days);

    // Bound the window with SQLite's own date math so it matches the
    // `date(...)` keys the aggregator wrote.
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&state.chat)
        .await?;
    let from: String = sqlx::query_scalar("SELECT date('now', ?1)")
        .bind(format!("-{} days", range_days - 1))
        .fetch_one(&state.chat)
        .await?;

    use db::analytics::{
        METRIC_ACTIVE_ROOMS, METRIC_DAU, METRIC_MAU, METRIC_MESSAGES, METRIC_SIGNUPS,
    };
    let specs: [(&'static str, &str, &str, bool); 5] = [
        ("Messages per day", METRIC_MESSAGES, "#2563eb", false),
        ("Daily active users", METRIC_DAU, "#16a34a", true),
        ("Monthly active users", METRIC_MAU, "#9333ea", true),
        ("Active rooms", METRIC_ACTIVE_ROOMS, "#ea580c", true),
        ("New signups", METRIC_SIGNUPS, "#0891b2", false),
    ];

    let mut cards: Vec<MetricCard> = Vec::with_capacity(specs.len());
    for (label, metric, color, is_snapshot) in specs {
        let points = db::analytics::series(&state.chat, metric, &from, &today).await?;
        let latest = points.last().map(|p| p.value).unwrap_or(0);
        let total = points.iter().map(|p| p.value).sum();
        let chart_svg = charts::line_chart(&points, color);
        cards.push(MetricCard {
            label,
            latest,
            total,
            is_snapshot,
            chart_svg,
        });
    }

    const RETENTION_WEEKS: usize = 8;
    let retention = db::analytics::retention(&state.auth, &state.chat, RETENTION_WEEKS).await?;
    let retention_headers: Vec<String> = (0..RETENTION_WEEKS).map(|k| format!("W{k}")).collect();

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let page = AnalyticsPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "analytics",
        range_days,
        today: &today,
        cards: &cards,
        retention_headers: &retention_headers,
        retention: &retention,
    };
    html(&page)
}

/// LC-97: recompute today's metrics on demand, then redirect back to the
/// dashboard preserving the selected range.
pub async fn post_recompute_analytics(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    Form(form): Form<RecomputeForm>,
) -> Result<Response, AppError> {
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&state.chat)
        .await?;
    db::analytics::recompute_day(&state.auth, &state.chat, &today).await?;
    let range_days = normalize_range(form.days);
    Ok(Redirect::to(&format!("/admin/analytics?days={range_days}")).into_response())
}

// LC-76: custom slash commands -------------------------------------------

#[derive(Deserialize)]
pub struct SlashCommandForm {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    pub target: String,
    #[serde(default)]
    pub admin_only: Option<String>,
}

async fn render_slash_commands_page(
    state: &AppState,
    user: &crate::models::User,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let builtins: Vec<BuiltinCommandRowView> = crate::commands::BUILTINS
        .iter()
        .map(|b| BuiltinCommandRowView {
            usage: b.usage.to_string(),
            description: b.description.to_string(),
        })
        .collect();
    let commands: Vec<SlashCommandRowView> = db::slash::list_global(&state.chat)
        .await?
        .into_iter()
        .map(|c| SlashCommandRowView {
            id: c.id,
            name: c.name,
            description: c.description,
            kind: c.kind.as_str().to_string(),
            target: c.target,
            admin_only: c.admin_only,
        })
        .collect();
    let page = SlashCommandsPage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "slash_commands",
        builtins: &builtins,
        commands: &commands,
        error,
    };
    html(&page)
}

pub async fn get_slash_commands(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_slash_commands_page(&state, &user, None).await
}

pub async fn post_slash_commands(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<SlashCommandForm>,
) -> Result<Response, AppError> {
    let name = form
        .name
        .trim()
        .trim_start_matches('/')
        .to_ascii_lowercase();
    let err = |msg: &str| msg.to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(render_slash_commands_page(
            &state,
            &actor,
            Some(err("Name must be non-empty letters, digits, - or _.")),
        )
        .await?
        .into_response());
    }
    if crate::commands::find_builtin(&name).is_some() {
        return Ok(render_slash_commands_page(
            &state,
            &actor,
            Some(err("That name is a built-in command.")),
        )
        .await?
        .into_response());
    }
    let Some(kind) = db::slash::CustomKind::parse(&form.kind) else {
        return Ok(
            render_slash_commands_page(&state, &actor, Some(err("Unknown kind.")))
                .await?
                .into_response(),
        );
    };
    let target = form.target.trim();
    if target.is_empty() {
        return Ok(
            render_slash_commands_page(&state, &actor, Some(err("Target is required.")))
                .await?
                .into_response(),
        );
    }
    if kind == db::slash::CustomKind::WebhookPost && !super::slash::webhook_url_ok(target) {
        return Ok(render_slash_commands_page(
            &state,
            &actor,
            Some(err(
                "Webhook target must be a public http(s) URL (no localhost or private IPs).",
            )),
        )
        .await?
        .into_response());
    }
    let kind_str = kind.as_str();
    match db::slash::insert_global(
        &state.chat,
        &name,
        form.description.trim(),
        kind,
        target,
        form.admin_only.is_some(),
        &actor.id,
    )
    .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Ok(render_slash_commands_page(
                &state,
                &actor,
                Some(err("A command with that name already exists.")),
            )
            .await?
            .into_response());
        }
        Err(e) => return Err(AppError::from(e)),
    }
    db::moderation::log_mod_action(
        &state.chat,
        "slash_command_add",
        "",
        &actor.id,
        Some(&name),
        None,
        Some(kind_str),
    )
    .await?;
    Ok(Redirect::to("/admin/slash-commands").into_response())
}

pub async fn post_slash_commands_delete(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::slash::delete(&state.chat, id).await?;
    db::moderation::log_mod_action(
        &state.chat,
        "slash_command_remove",
        "",
        &actor.id,
        None,
        None,
        None,
    )
    .await?;
    Ok(Redirect::to("/admin/slash-commands"))
}

// LC-73: bot accounts ----------------------------------------------------

#[derive(Deserialize)]
pub struct BotCreateForm {
    pub username: String,
    #[serde(default)]
    pub s_messages_read: Option<String>,
    #[serde(default)]
    pub s_messages_write: Option<String>,
    #[serde(default)]
    pub s_rooms_read: Option<String>,
}

async fn render_bots_page(
    state: &AppState,
    user: &crate::models::User,
    new_token: Option<String>,
    new_bot_name: Option<String>,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let bots: Vec<BotRowView> = db::auth::list_bots(&state.auth)
        .await?
        .into_iter()
        .map(|b| BotRowView {
            id: b.id,
            username: b.username,
            disabled: b.is_banned,
            created_at: b.created_at,
        })
        .collect();
    let page = BotsPage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "bots",
        available: state.secret_key.is_some(),
        all_scopes: super::api::ALL_SCOPES,
        bots: &bots,
        new_token,
        new_bot_name,
        error,
    };
    html(&page)
}

pub async fn get_bots(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_bots_page(&state, &user, None, None, None).await
}

pub async fn post_bots(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<BotCreateForm>,
) -> Result<Response, AppError> {
    let Some(secret) = state.secret_key.as_ref() else {
        return Ok(render_bots_page(
            &state,
            &actor,
            None,
            None,
            Some("Bots need an API token, which requires a server secret key (LETS_CHAT_SECRET_KEY).".into()),
        )
        .await?
        .into_response());
    };
    let username = form.username.trim();
    if username.is_empty() {
        return Ok(render_bots_page(
            &state,
            &actor,
            None,
            None,
            Some("Bot username is required.".into()),
        )
        .await?
        .into_response());
    }
    let mut scopes: Vec<&str> = Vec::new();
    if form.s_messages_read.is_some() {
        scopes.push(super::api::SCOPE_MESSAGES_READ);
    }
    if form.s_messages_write.is_some() {
        scopes.push(super::api::SCOPE_MESSAGES_WRITE);
    }
    if form.s_rooms_read.is_some() {
        scopes.push(super::api::SCOPE_ROOMS_READ);
    }
    if scopes.is_empty() {
        return Ok(render_bots_page(
            &state,
            &actor,
            None,
            None,
            Some("Select at least one scope for the bot's token.".into()),
        )
        .await?
        .into_response());
    }
    let scopes = scopes.join(" ");

    let bot_id = match db::auth::create_bot(&state.auth, username).await {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Ok(render_bots_page(
                &state,
                &actor,
                None,
                None,
                Some("That username is already taken.".into()),
            )
            .await?
            .into_response());
        }
        Err(e) => return Err(AppError::from(e)),
    };
    // Backfill the bot into the General enclave so it can be added to rooms.
    if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
        tracing::warn!(error = %e, "bot general backfill failed");
    }
    let plaintext = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(secret, &plaintext);
    // Roll back the bot row if token minting fails, so a failed create does
    // not leave an orphan bot that blocks retrying the same username.
    if let Err(e) =
        db::api_tokens::insert(&state.auth, &bot_id, "bot token", &hash, &scopes, None).await
    {
        let _ = db::auth::delete_user(&state.auth, &bot_id).await;
        return Err(AppError::from(e));
    }
    db::moderation::log_mod_action(
        &state.chat,
        "bot_create",
        &bot_id,
        &actor.id,
        Some(username),
        None,
        Some(&scopes),
    )
    .await?;
    Ok(render_bots_page(
        &state,
        &actor,
        Some(plaintext),
        Some(username.to_string()),
        None,
    )
    .await?
    .into_response())
}

/// POST /admin/bots/{id}/disable - ban the bot and revoke all its API
/// tokens (LC-73). The id is the bot user's UUID.
pub async fn post_bot_disable(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<String>,
) -> Result<Redirect, AppError> {
    // Only act on actual bot rows so this cannot ban a human.
    let is_bot = db::auth::find_user_by_id(&state.auth, &id)
        .await?
        .map(|u| u.is_bot)
        .unwrap_or(false);
    if is_bot {
        db::auth::ban_user(&state.auth, &id, Some("bot disabled")).await?;
        let revoked = db::api_tokens::revoke_all_for_user(&state.auth, &id).await?;
        db::moderation::log_mod_action(
            &state.chat,
            "bot_disable",
            &id,
            &actor.id,
            None,
            None,
            Some(&format!("revoked {revoked} tokens")),
        )
        .await?;
    }
    Ok(Redirect::to("/admin/bots"))
}

// LC-75: outgoing webhooks ------------------------------------------------

#[derive(Deserialize)]
pub struct OutgoingCreateForm {
    pub scope_kind: String,
    #[serde(default)]
    pub scope_id: Option<i64>,
    pub url: String,
    #[serde(default)]
    pub e_message_posted: Option<String>,
    #[serde(default)]
    pub e_message_edited: Option<String>,
    #[serde(default)]
    pub e_message_deleted: Option<String>,
    #[serde(default)]
    pub e_reaction_added: Option<String>,
}

fn scope_label(kind: &str, id: Option<i64>) -> String {
    match (kind, id) {
        ("global", _) => "global".to_string(),
        (k, Some(i)) => format!("{k} #{i}"),
        (k, None) => k.to_string(),
    }
}

async fn render_outgoing_page(
    state: &AppState,
    user: &crate::models::User,
    revealed: Option<(i64, String)>,
    error: Option<String>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, None).await?;
    let webhooks: Vec<OutgoingWebhookRowView> = db::outgoing_webhooks::list_all(&state.chat)
        .await?
        .into_iter()
        .map(|w| OutgoingWebhookRowView {
            id: w.id,
            scope: scope_label(&w.scope_kind, w.scope_id),
            events: w.events,
            url: w.url,
            signing_secret: w.signing_secret,
            created_at: w.created_at,
            last_success_at: w.last_success_at,
            last_failure_at: w.last_failure_at,
            consecutive_failures: w.consecutive_failures,
            disabled: w.disabled_at.is_some(),
        })
        .collect();
    let page = OutgoingWebhooksPage {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "outgoing_webhooks",
        all_events: OUTGOING_EVENTS,
        webhooks: &webhooks,
        revealed,
        error,
    };
    html(&page)
}

pub async fn get_outgoing_webhooks(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_outgoing_page(&state, &user, None, None).await
}

pub async fn post_outgoing_webhooks(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    axum::Form(form): axum::Form<OutgoingCreateForm>,
) -> Result<Response, AppError> {
    let err = |state: &AppState, actor: &crate::models::User, msg: &str| {
        let msg = msg.to_string();
        let state = state.clone();
        let actor = actor.clone();
        async move {
            render_outgoing_page(&state, &actor, None, Some(msg))
                .await
                .map(IntoResponse::into_response)
        }
    };

    let url = form.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return err(&state, &actor, "URL must be an http(s) URL.").await;
    }
    let scope_kind = form.scope_kind.as_str();
    let scope_id = match scope_kind {
        "global" => None,
        "enclave" | "room" => match form.scope_id {
            Some(id) => Some(id),
            None => return err(&state, &actor, "Enclave/room scope needs a scope id.").await,
        },
        _ => return err(&state, &actor, "Invalid scope.").await,
    };
    let mut events: Vec<&str> = Vec::new();
    if form.e_message_posted.is_some() {
        events.push("message.posted");
    }
    if form.e_message_edited.is_some() {
        events.push("message.edited");
    }
    if form.e_message_deleted.is_some() {
        events.push("message.deleted");
    }
    if form.e_reaction_added.is_some() {
        events.push("reaction.added");
    }
    if events.is_empty() {
        return err(&state, &actor, "Select at least one event.").await;
    }
    let events = events.join(" ");

    let secret = crate::auth::generate_api_token();
    let id = db::outgoing_webhooks::insert(
        &state.chat,
        scope_kind,
        scope_id,
        &events,
        url,
        &secret,
        &actor.id,
    )
    .await?;
    db::moderation::log_mod_action(
        &state.chat,
        "outgoing_webhook_create",
        "",
        &actor.id,
        Some(&scope_label(scope_kind, scope_id)),
        None,
        Some(&events),
    )
    .await?;
    Ok(
        render_outgoing_page(&state, &actor, Some((id, secret)), None)
            .await?
            .into_response(),
    )
}

pub async fn post_outgoing_rotate(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    let secret = crate::auth::generate_api_token();
    db::outgoing_webhooks::rotate_secret(&state.chat, id, &secret).await?;
    render_outgoing_page(&state, &actor, Some((id, secret)), None).await
}

#[derive(Deserialize)]
pub struct ToggleForm {
    #[serde(default)]
    pub enable: Option<String>,
}

pub async fn post_outgoing_toggle(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<ToggleForm>,
) -> Result<Redirect, AppError> {
    db::outgoing_webhooks::set_enabled(&state.chat, id, form.enable.is_some()).await?;
    Ok(Redirect::to("/admin/outgoing-webhooks"))
}

pub async fn post_outgoing_delete(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::outgoing_webhooks::delete(&state.chat, id).await?;
    Ok(Redirect::to("/admin/outgoing-webhooks"))
}

pub async fn get_outgoing_deliveries(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let deliveries: Vec<DeliveryRowView> =
        db::outgoing_webhooks::deliveries_for(&state.chat, id, 50)
            .await?
            .into_iter()
            .map(|d| DeliveryRowView {
                id: d.id,
                event: d.event,
                attempt: d.attempt,
                status: d.status,
                scheduled_at: d.scheduled_at,
                delivered_at: d.delivered_at,
            })
            .collect();
    html(&OutgoingWebhookDeliveriesPage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "outgoing_webhooks",
        webhook_id: id,
        deliveries: &deliveries,
    })
}
