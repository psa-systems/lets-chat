use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

use crate::auth::AdminUser;
use crate::db;
use crate::error::AppError;
use crate::i18n::translate_current;
use crate::state::AppState;
use crate::version;
use crate::views::admin::{
    AdminEnclaveView, AdminInviteView, AdminRoomView, AdminUserView, AnalyticsPage, AntiSpamPage,
    BackupRestorePage, BotRowView, BotsPage, BrandingPage, BridgeAvatarFailureView,
    BridgeAvatarStatsView, BridgeAvatarsPage, BridgeRowView, BridgesPage, BuiltinCommandRowView,
    DeliveryRowView, DropView, EnclavesPage, ImapPollStatusView, InvitesPage, LinkFilterPage,
    LinkFilterRuleView, MetricCard, ModLogPage, OutgoingWebhookDeliveriesPage,
    OutgoingWebhookRowView, OutgoingWebhooksPage, QuarantineEntryView, QuarantinePage, ReportsPage,
    RetentionStatusView, RoomRowFragment, RoomsPage, SettingsPage, SlashCommandRowView,
    SlashCommandsPage, UserRowFragment, UsersPage, OUTGOING_EVENTS,
};
use crate::views::charts;
use crate::views::settings::SettingsFeedback;
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

/// LC-510: true when the request came from htmx, so a mutating admin handler
/// returns the shared `SettingsFeedback` status fragment (inline status +
/// toast) instead of a full-page redirect. Mirrors `routes::settings::is_hx`.
fn is_hx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin", get(get_settings))
        .route("/admin/settings", get(get_settings).post(post_settings))
        .route("/admin/settings/imap", post(post_imap_settings))
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
        // LC-334: message-report review queue.
        .route("/admin/reports", get(get_reports))
        .route("/admin/reports/badge", get(get_reports_badge))
        .route("/admin/reports/{id}/resolve", post(post_report_resolve))
        .route("/admin/reports/{id}/dismiss", post(post_report_dismiss))
        .route("/admin/bots", get(get_bots).post(post_bots))
        .route("/admin/bots/{id}/disable", post(post_bot_disable))
        .route("/admin/bridges", get(get_bridges).post(post_bridges))
        .route("/admin/bridges/avatars", get(get_bridge_avatars))
        .route("/admin/bridges/{id}/remove", post(post_bridge_remove))
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

// LC-77-SMTP-SEAL: the SMTP fields used to live here (smtp_host, _port,
// _user, _pass, _from) and got persisted to `settings.db` by `post_settings`.
// They were write-only - no code read them back, because
// `mail::Mailer::from_env` is the actual SMTP loader and reads exclusively
// from the `LETS_CHAT_SMTP_*` env vars. The admin form was misleading UX scaffolding;
// the password sitting in `settings.db` plaintext was the only real
// consequence. The form is gone; SMTP is documented as env-var-only in
// `README.md`. Existing settings.db rows are dropped by migration
// `settings/0006_drop_smtp_settings.sql`.
//
// `SettingsForm` is retained as an empty struct so the `axum::Form` extractor
// on `post_settings` still has a target; it has no fields. Future
// non-SMTP knobs land here.
#[derive(Deserialize, Default)]
pub struct SettingsForm {}

#[derive(Deserialize, Default)]
pub struct SettingsQuery {
    pub regenerated: Option<i64>,
    pub purged: Option<i64>,
    /// Set after a successful /admin/settings/imap POST so the settings
    /// page renders the IMAP-saved confirmation banner under that section
    /// (distinct from the SMTP `saved` banner, which uses the in-place
    /// render flag instead of a query param).
    pub imap_saved: Option<i64>,
}

#[derive(Deserialize)]
pub struct ImapSettingsForm {
    pub imap_host: String,
    pub imap_port: String,
    pub imap_username: String,
    /// Write-only password input. Empty string preserves the existing
    /// sealed value; non-empty re-seals and overwrites.
    pub imap_password: String,
    pub imap_folder: String,
    pub imap_ingress_domain: String,
    /// LC-77-DEAD-LETTER: optional IMAP folder name that dropped
    /// messages are UID COPYd into before being marked Seen on the
    /// source UID. Empty preserves the v1 always-Seen posture.
    #[serde(default)]
    pub imap_dead_letter_folder: String,
    /// Form-checkbox convention: present + non-empty when checked, absent
    /// when unchecked.
    #[serde(default)]
    pub imap_tls: Option<String>,
    #[serde(default)]
    pub imap_enabled: Option<String>,
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
    let (
        imap_host,
        imap_port,
        imap_tls,
        imap_username,
        imap_password_configured,
        imap_folder,
        imap_ingress_domain,
        imap_dead_letter_folder,
        imap_enabled,
    ) = load_imap_settings_for_display(&state).await;
    // LC-207-OBSERVABILITY (#278): poll-loop health + recent drops + retention
    // sweep status, surfaced read-only next to the config they describe.
    let imap_poll_status =
        db::imap_poll_status::read(&state.settings)
            .await?
            .map(|s| ImapPollStatusView {
                last_poll_at: s.last_poll_at,
                last_ok_at: s.last_ok_at,
                last_error: s.last_error,
                consecutive_failures: s.consecutive_failures,
                last_fetched: s.last_fetched,
                last_posted: s.last_posted,
                last_dropped: s.last_dropped,
            });
    let imap_drop_counts = db::email_ingress_drops::counts_by_reason(&state.chat, 24).await?;
    let imap_recent_drops = db::email_ingress_drops::recent(&state.chat, 20)
        .await?
        .into_iter()
        .map(|d| DropView {
            dropped_at: d.dropped_at,
            reason: d.reason,
            uid: d.uid,
            detail: d.detail,
        })
        .collect();
    let retention_sweep_enabled = crate::retention::sweep::flag_enabled();
    let retention_status =
        db::retention_status::read(&state.chat)
            .await?
            .map(|s| RetentionStatusView {
                last_run_at: s.last_run_at,
                last_rooms_touched: s.last_rooms_touched,
                last_messages_deleted: s.last_messages_deleted,
                total_messages_deleted: s.total_messages_deleted,
                runs: s.runs,
                last_error: s.last_error,
            });
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
        default_notify_email_digest,
        maintenance_enabled,
        maintenance_message,
        uploads_total_display,
        uploads_orphan_count,
        regenerated: q.regenerated,
        purged: q.purged,
        imap_host,
        imap_port,
        imap_tls,
        imap_username,
        imap_password_configured,
        imap_folder,
        imap_ingress_domain,
        imap_dead_letter_folder,
        imap_enabled,
        imap_error: None,
        imap_saved: q.imap_saved.is_some(),
        imap_poll_status,
        imap_drop_counts,
        imap_recent_drops,
        retention_sweep_enabled,
        retention_status,
    };
    html(&page)
}

/// Read the singleton imap_inbox_config row for display. `secret_key` is
/// needed only to decrypt the password byte length; we expose
/// `imap_password_configured: bool` to the template, never the plaintext.
/// Falls back to defaults on missing row OR missing secret_key OR decrypt
/// failure (so an admin who rotated the secret key after writing IMAP
/// creds at least sees the page; they will need to re-enter the password
/// before the spawn gate succeeds again).
async fn load_imap_settings_for_display(
    state: &AppState,
) -> (
    String,
    String,
    bool,
    String,
    bool,
    String,
    String,
    String,
    bool,
) {
    let defaults = || {
        (
            String::new(),
            "993".to_string(),
            true,
            String::new(),
            false,
            "INBOX".to_string(),
            String::new(),
            String::new(),
            false,
        )
    };
    let Some(key) = state.secret_key.as_ref() else {
        // No secret key -> no sealed creds readable. All fields default
        // to empty and the template surfaces a missing-LETS_CHAT_SECRET_KEY
        // help line via the existing 2FA-style affordance.
        return defaults();
    };
    match db::imap_config::read(&state.settings, key.as_ref()).await {
        Ok(Some(cfg)) => (
            cfg.host,
            cfg.port.to_string(),
            cfg.tls,
            cfg.username,
            !cfg.password.is_empty(),
            cfg.folder,
            cfg.ingress_domain.unwrap_or_default(),
            cfg.dead_letter_folder.unwrap_or_default(),
            cfg.enabled,
        ),
        // A missing row (Ok(None)) is a fresh install; render empty
        // defaults so the operator can fill the form for the first time.
        Ok(None) => defaults(),
        // A decrypt failure (e.g. rotated secret key) is rare; surface
        // the form with empty fields so the admin can re-enter creds. A
        // future commit could add a banner; for v1 the spawn-loop log
        // already tells the operator the IMAP poll is disabled.
        Err(_) => defaults(),
    }
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
    headers: HeaderMap,
) -> Result<Response, AppError> {
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
        // LC-206: preview regen routes through the shared safe helper
        // (spawn_blocking + JoinError panic boundary). Behavior preserved:
        // a normal pipeline failure warns + continues the batch, a decoder
        // panic propagates as 500 (the old code's `?` on the JoinError) -
        // split explicitly in the match below.
        let preview_result = crate::uploads::pipeline::process_preview_safely(
            original_path.clone(),
            row.mime_type.clone(),
        )
        .await;
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
            Err(crate::uploads::pipeline::SafeImageError::Pipeline(e)) => {
                tracing::warn!(
                    error = %e,
                    row_id = row.id,
                    "regenerate: pipeline failed",
                );
            }
            Err(crate::uploads::pipeline::SafeImageError::Panic(e)) => {
                return Err(AppError::Internal(format!("regen decoder panic: {e}")));
            }
        }
    }
    tracing::info!(regenerated, "admin regenerate-thumbnails complete");
    if is_hx(&headers) {
        let msg = format!(
            "{}: {regenerated}",
            translate_current("admin-uploads-regenerated")
        );
        return Ok(html(&SettingsFeedback::ok(msg))?.into_response());
    }
    Ok(Redirect::to(&format!("/admin/settings?regenerated={regenerated}")).into_response())
}

/// Run the orphan sweeper with threshold = 0, i.e. consider every
/// `message_id IS NULL` row a candidate regardless of age. Reuses
/// `run_orphan_sweep` so the same dedup-aware transaction and the same
/// missing-file-is-success semantics apply; the only difference vs the
/// hourly tick is the threshold.
pub async fn post_purge_orphans(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let stats = crate::uploads::sweep::run_orphan_sweep(&state.chat, 0)
        .await
        .map_err(|e| AppError::Internal(format!("purge orphans: {e}")))?;
    tracing::info!(
        rows = stats.rows_deleted,
        files = stats.files_deleted,
        errors = stats.errors,
        "admin purge-orphans complete",
    );
    if is_hx(&headers) {
        let msg = format!(
            "{}: {}",
            translate_current("admin-uploads-purged"),
            stats.rows_deleted
        );
        return Ok(html(&SettingsFeedback::ok(msg))?.into_response());
    }
    Ok(Redirect::to(&format!("/admin/settings?purged={}", stats.rows_deleted)).into_response())
}

/// LC-77-SMTP-SEAL: previously wrote the SMTP form into `settings.db`
/// (host/port/user/from plain, password plain), but `mail::Mailer::from_env`
/// is the actual SMTP loader and reads exclusively from `LETS_CHAT_SMTP_*` env vars,
/// so the writes were dead storage. The form is gone; this handler is
/// retained as an empty no-op redirect so a future non-SMTP settings field
/// has somewhere to land without re-wiring the route. The route itself
/// stays registered so a stale browser POST does not 404.
pub async fn post_settings(
    State(_state): State<AppState>,
    AdminUser(_user): AdminUser,
    axum::Form(_form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    Ok(Redirect::to("/admin/settings").into_response())
}

/// LC-77: save the IMAP-poll config. Routes through `db::imap_config::write`
/// so the password is AES-256-GCM-sealed under the process secret key. An
/// empty password input preserves the existing sealed value; a non-empty
/// input re-seals and overwrites.
pub async fn post_imap_settings(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    headers: HeaderMap,
    Form(form): Form<ImapSettingsForm>,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref() else {
        let msg = "IMAP settings require LETS_CHAT_SECRET_KEY to be set on the server".to_string();
        if is_hx(&headers) {
            return Ok(html(&SettingsFeedback::err(msg))?.into_response());
        }
        return Err(AppError::BadRequest(msg));
    };
    let port: u16 = match form.imap_port.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            let msg = format!(
                "IMAP port must be a 1-65535 integer; got {:?}",
                form.imap_port
            );
            if is_hx(&headers) {
                return Ok(html(&SettingsFeedback::err(msg))?.into_response());
            }
            return Err(AppError::BadRequest(msg));
        }
    };
    let host = form.imap_host.trim().to_string();
    let username = form.imap_username.trim().to_string();
    let folder = {
        let f = form.imap_folder.trim();
        if f.is_empty() {
            "INBOX".to_string()
        } else {
            f.to_string()
        }
    };
    let ingress_domain = {
        let d = form.imap_ingress_domain.trim();
        if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        }
    };
    let dead_letter_folder = {
        let d = form.imap_dead_letter_folder.trim();
        if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        }
    };
    let tls = form.imap_tls.is_some();
    let enabled = form.imap_enabled.is_some();

    // Empty password input = preserve existing sealed value. Read the
    // existing row to recover the plaintext password we just decrypted
    // for display, then write it back through the same seal/open path.
    // If the row does not exist yet, an empty input means "no password"
    // which the spawn gate will refuse to use anyway.
    let password = if form.imap_password.is_empty() {
        match db::imap_config::read(&state.settings, key.as_ref()).await {
            Ok(Some(existing)) => existing.password,
            _ => String::new(),
        }
    } else {
        form.imap_password
    };

    let cfg = crate::db::imap_config::ImapConfig {
        host,
        port,
        tls,
        username,
        password,
        folder,
        ingress_domain,
        enabled,
        dead_letter_folder,
    };
    db::imap_config::write(&state.settings, key.as_ref(), &cfg)
        .await
        .map_err(|e| AppError::Internal(format!("imap_config write: {e}")))?;

    if is_hx(&headers) {
        return Ok(html(&SettingsFeedback::ok(translate_current("admin-saved")))?.into_response());
    }
    Ok(Redirect::to("/admin/settings?imap_saved=1").into_response())
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
    headers: HeaderMap,
    axum::Form(form): axum::Form<EmailDigestDefaultForm>,
) -> Result<Response, AppError> {
    let value = if form.default_notify_email_digest.is_some() {
        "1"
    } else {
        "0"
    };
    db::settings::set_setting(&state.settings, "default_notify_email_digest", value).await?;
    if is_hx(&headers) {
        return Ok(html(&SettingsFeedback::ok(translate_current("admin-saved")))?.into_response());
    }
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
    headers: HeaderMap,
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
    if is_hx(&headers) {
        return Ok(html(&SettingsFeedback::ok(translate_current("admin-saved")))?.into_response());
    }
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

/// LC-352: refuse an action that would strip admin capability from the last
/// active admin (role=admin, not banned) and lock everyone out of /admin.
/// `target_id` is the user the action removes admin power from. A no-op when the
/// target is not an active admin or another active admin remains.
async fn guard_not_last_admin(state: &AppState, target_id: &str) -> Result<(), AppError> {
    let Some(target) = db::auth::find_user_by_id(&state.auth, target_id).await? else {
        return Ok(());
    };
    let target_is_active_admin = target.role == "admin" && !target.is_banned;
    if target_is_active_admin
        && !db::auth::other_active_admin_exists(&state.auth, target_id).await?
    {
        return Err(AppError::BadRequest(
            "cannot remove the last admin; promote another admin first".into(),
        ));
    }
    Ok(())
}

pub async fn post_ban(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(user_id): Path<String>,
) -> Result<Html, AppError> {
    guard_not_last_admin(&state, &user_id).await?;
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
    // LC-352: demoting away from admin must not remove the last active admin.
    if role != "admin" {
        guard_not_last_admin(&state, &user_id).await?;
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
    // LC-352: deleting the last active admin would lock everyone out of /admin.
    guard_not_last_admin(&state, &user_id).await?;
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
    // LC-175: drop the row from every other admin's open user list.
    state.hub.broadcast_to_topic(
        "admin",
        &ChatEvent::AdminUserChanged {
            user_id: user_id.clone(),
            removed: true,
        },
    );
    Ok(Html(String::new()))
}

/// Build the admin-list view-row for one user, or None if the user is gone.
/// Shared by the actor's HTTP row response and the LC-175 `admin`-topic WS
/// broadcast so both render an identical row.
pub(crate) async fn build_admin_user_view(
    state: &AppState,
    user_id: &str,
) -> Result<Option<AdminUserView>, AppError> {
    let Some(record) = db::auth::find_user_by_id(&state.auth, user_id).await? else {
        return Ok(None);
    };
    let usage_bytes = db::quota::sum_user_usage(&state.chat, &record.id).await?;
    let quota_bytes = db::quota::get_user_quota(&state.chat, &record.id).await?;
    Ok(Some(AdminUserView {
        id: record.id,
        username: record.username,
        role: record.role,
        is_banned: record.is_banned,
        is_muted: record.is_muted,
        usage_display: format_bytes_mib(usage_bytes),
        quota_mib_value: bytes_to_mib_input(quota_bytes),
    }))
}

/// Render the actor's HTTP row response AND broadcast the same change to the
/// `admin` topic so every other admin's open user list updates the row live
/// (LC-175). Call after any mutation that changes a still-existing user row.
async fn render_user_row(state: &AppState, user_id: &str) -> Result<Html, AppError> {
    let view = build_admin_user_view(state, user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    state.hub.broadcast_to_topic(
        "admin",
        &ChatEvent::AdminUserChanged {
            user_id: user_id.to_string(),
            removed: false,
        },
    );
    html(&UserRowFragment {
        u: &view,
        oob: false,
    })
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
    // LC-177: drop the row from every other admin's open room list.
    state.hub.broadcast_to_topic(
        "admin",
        &ChatEvent::AdminRoomChanged {
            room_id,
            removed: true,
        },
    );
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

/// Build the admin-list view-row for one room, or None if the room is gone.
/// Shared by the actor's HTTP row response and the LC-177 `admin`-topic WS
/// broadcast so both render an identical row.
pub(crate) async fn build_admin_room_view(
    state: &AppState,
    room_id: i64,
) -> Result<Option<AdminRoomView>, AppError> {
    let Some(r) = db::chat::get_room(&state.chat, room_id).await? else {
        return Ok(None);
    };
    let members = db::chat::count_room_members(&state.chat, room_id).await?;
    Ok(Some(AdminRoomView {
        id: r.id,
        name: r.name,
        topic: r.topic,
        room_type: r.room_type,
        invite_code: r.invite_code,
        members,
        created_at: r.created_at,
    }))
}

/// Render the actor's HTTP row response AND broadcast the same change to the
/// `admin` topic so every other admin's open room list updates the row live
/// (LC-177). Call after any mutation that changes a still-existing room row.
async fn render_room_row(state: &AppState, room_id: i64) -> Result<Html, AppError> {
    let view = build_admin_room_view(state, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    state.hub.broadcast_to_topic(
        "admin",
        &ChatEvent::AdminRoomChanged {
            room_id,
            removed: false,
        },
    );
    html(&RoomRowFragment {
        r: &view,
        oob: false,
    })
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

// Report queue (LC-334) --------------------------------------------------------

/// `GET /admin/reports` - the open message-report review queue.
pub async fn get_reports(
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
    let reports = super::report::build_report_views(&state).await?;
    let page = ReportsPage {
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
        section: "reports",
        reports: &reports,
    };
    html(&page)
}

/// `GET /admin/reports/badge` - the nav open-count badge inner markup, fetched
/// on load by every admin page (so the count need not thread through every admin
/// page struct).
pub async fn get_reports_badge(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
) -> Result<Html, AppError> {
    let open_count = db::reports::count_open(&state.chat).await?;
    html(&crate::views::report::AdminReportsBadge { open_count })
}

/// `POST /admin/reports/{id}/resolve`
pub async fn post_report_resolve(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    handle_report_status(&state, &user.id, id, "resolved", "report_resolved").await
}

/// `POST /admin/reports/{id}/dismiss`
pub async fn post_report_dismiss(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    handle_report_status(&state, &user.id, id, "dismissed", "report_dismissed").await
}

/// Transition an open report to `status`, audit it in the mod log, and live-
/// update every admin queue. Returns the OOB fragment so the acting admin's
/// queue + nav badge update from the HTTP response even if their WS lags. A
/// report already handled by another admin is a no-op (no second audit row).
async fn handle_report_status(
    state: &AppState,
    actor_id: &str,
    report_id: i64,
    status: &str,
    action: &str,
) -> Result<Html, AppError> {
    let updated = db::reports::set_status(&state.chat, report_id, status, actor_id).await?;
    if updated {
        db::moderation::log_mod_action(
            &state.chat,
            action,
            &format!("report#{report_id}"),
            actor_id,
            None,
            None,
            None,
        )
        .await?;
        super::report::broadcast_reports_changed(state);
    }
    super::report::render_reports_oob(state).await
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
/// LC-358: upper bounds so an operator typo (or a forged POST past the input's
/// `max`) cannot store an absurd value. The per-enclave message-rate burst is
/// already capped at 10000; mirror that for the site-wide cap. The quota cap is
/// generous (16 TiB) but finite.
const MAX_RATE_LIMIT_MESSAGES: u32 = 10_000;
const MAX_QUOTA_MIB: i64 = 16 * 1024 * 1024;

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
    if n > MAX_QUOTA_MIB {
        return Err(AppError::BadRequest(format!(
            "quota too large (max {MAX_QUOTA_MIB} MiB)"
        )));
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
            // LC-358: clamp to the same ceiling as the per-enclave burst.
            .map(|n| n.min(MAX_RATE_LIMIT_MESSAGES).to_string())
            .unwrap_or_else(|| "0".to_string())
    }
    let msg = cap_or_zero(&form.rate_limit_messages);
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
    db::settings::set_setting(&state.settings, "link_filter_enabled", link).await?;
    db::settings::set_setting(&state.settings, "honeypot_enabled", hp).await?;
    let summary = format!("msg={msg} link={link} hp={hp}");
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
                email_inbox_id: raw.email_inbox_id,
                bridge_id: raw.bridge_id,
                bridge_foreign_name: raw.bridge_foreign_name,
                bridge_kind: raw.bridge_kind,
                bridge_foreign_avatar: raw.bridge_foreign_avatar,
            };
            let event = ChatEvent::NewMessage {
                message,
                is_dm: room.room_type == "dm",
                client_id: None,
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

// LC-78: bridges -----------------------------------------------------------

/// A bridge is considered `stale` when no heartbeat has been recorded for
/// this many seconds. Set to 3x the typical 30s daemon heartbeat interval.
/// The daemon's actual heartbeat cadence is up to the operator; this is the
/// admin UI's threshold for surfacing "the daemon may have died."
const BRIDGE_HEARTBEAT_STALE_SECS: i64 = 90;

/// Derive the display status from the stored row. The daemon writes
/// `healthy` / `errored` via the heartbeat endpoint; `pending` and `stale`
/// are computed here so the DB schema does not need a polling sweeper.
fn derive_bridge_status(b: &crate::models::Bridge) -> &'static str {
    let Some(ts) = b.last_heartbeat_at.as_deref() else {
        return "pending";
    };
    // SQLite's datetime('now') format: "YYYY-MM-DD HH:MM:SS" (UTC, no TZ).
    let parsed =
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc());
    let stale = match parsed {
        Ok(dt) => (chrono::Utc::now() - dt).num_seconds() > BRIDGE_HEARTBEAT_STALE_SECS,
        Err(_) => false,
    };
    if b.status == "errored" {
        "errored"
    } else if stale {
        "stale"
    } else {
        "healthy"
    }
}

#[derive(Deserialize)]
pub struct BridgeCreateForm {
    pub room_id: i64,
    pub bot_username: String,
    pub kind: String,
    pub config: String,
}

async fn render_bridges_page(
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
    let raw = db::bridges::list_all(&state.chat).await?;
    // Resolve room name + bot username for each bridge. Joins span two DBs
    // (rooms live in chat.db; users live in auth.db), so we run lookups per
    // row. The admin bridges list is expected to stay small.
    let mut bridges: Vec<BridgeRowView> = Vec::with_capacity(raw.len());
    for b in &raw {
        let room_name = db::chat::get_room(&state.chat, b.room_id)
            .await?
            .map(|r| r.name)
            .unwrap_or_else(|| format!("#{}", b.room_id));
        let bot_username = db::auth::find_user_by_id(&state.auth, &b.bot_user_id)
            .await?
            .map(|u| u.username)
            .unwrap_or_else(|| "(unknown)".to_string());
        bridges.push(BridgeRowView {
            id: b.id,
            room_id: b.room_id,
            room_name,
            kind: b.kind.clone(),
            bot_username,
            status: derive_bridge_status(b),
            last_heartbeat_at: b.last_heartbeat_at.clone(),
            last_error: b.last_error.clone(),
            created_at: b.created_at.clone(),
        });
    }
    // Room list for the create-form drop-down. Same admin-sees-all view as
    // the rooms page; excludes DMs (a bridge to a 1:1 room doesn't make sense).
    let admin_rooms_raw = db::chat::list_rooms(&state.chat, &user.id, true).await?;
    let rooms: Vec<AdminRoomView> = admin_rooms_raw
        .into_iter()
        .map(|r| AdminRoomView {
            id: r.id,
            name: r.name,
            topic: r.topic,
            room_type: r.room_type,
            invite_code: r.invite_code,
            members: 0,
            created_at: r.created_at,
        })
        .collect();
    let page = BridgesPage {
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
        section: "bridges",
        available: state.secret_key.is_some(),
        bridges: &bridges,
        rooms: &rooms,
        new_token,
        new_bot_name,
        error,
    };
    html(&page)
}

pub async fn get_bridges(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_bridges_page(&state, &user, None, None, None).await
}

/// POST /admin/bridges - create a bridge in one shot: mints the bot, sets
/// its role tier, mints the bridge-scoped API token, seals the daemon
/// config under LETS_CHAT_SECRET_KEY, and inserts the bridges row. The
/// bot is committed only if every step succeeds; the bot is rolled back
/// on partial failure so the admin can retry the same username.
pub async fn post_bridges(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Form(form): Form<BridgeCreateForm>,
) -> Result<Response, AppError> {
    let Some(secret) = state.secret_key.as_ref() else {
        return Ok(render_bridges_page(
            &state,
            &actor,
            None,
            None,
            Some("Bridges need a server secret key (LETS_CHAT_SECRET_KEY).".into()),
        )
        .await?
        .into_response());
    };
    let bot_username = form.bot_username.trim();
    let kind = form.kind.trim();
    let config = form.config.trim();
    if bot_username.is_empty() {
        return Ok(render_bridges_page(
            &state,
            &actor,
            None,
            None,
            Some("Bot username is required.".into()),
        )
        .await?
        .into_response());
    }
    // v1 ships Matrix only. The schema accepts any kind (TEXT, no CHECK), so
    // adding IRC/XMPP later is a code-level loosening; the gate is here.
    if kind != "matrix" {
        return Ok(render_bridges_page(
            &state,
            &actor,
            None,
            None,
            Some("Only the 'matrix' kind is supported in v1.".into()),
        )
        .await?
        .into_response());
    }
    if config.is_empty() {
        return Ok(render_bridges_page(
            &state,
            &actor,
            None,
            None,
            Some("Daemon config is required.".into()),
        )
        .await?
        .into_response());
    }
    // LC-357: the target room must exist and not be a DM (a bridge to a DM is
    // nonsensical). Validate before minting the bot so a bad room_id cannot
    // orphan a bot user + token.
    match db::chat::get_room(&state.chat, form.room_id).await? {
        Some(room) if room.room_type != "dm" => {}
        Some(_) => {
            return Ok(render_bridges_page(
                &state,
                &actor,
                None,
                None,
                Some("Cannot bridge a direct-message room.".into()),
            )
            .await?
            .into_response());
        }
        None => {
            return Ok(render_bridges_page(
                &state,
                &actor,
                None,
                None,
                Some("Selected room does not exist.".into()),
            )
            .await?
            .into_response());
        }
    }
    // 1. Mint bot user.
    let bot_id = match db::auth::create_bot(&state.auth, bot_username).await {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Ok(render_bridges_page(
                &state,
                &actor,
                None,
                None,
                Some("That bot username is already taken.".into()),
            )
            .await?
            .into_response());
        }
        Err(e) => return Err(AppError::from(e)),
    };
    // 2. Assign the narrow `bridge` role tier (chunk 5 / require_not_bridge).
    if let Err(e) = db::auth::set_user_role(&state.auth, &bot_id, db::auth::ROLE_BRIDGE).await {
        let _ = db::auth::delete_user(&state.auth, &bot_id).await;
        return Err(AppError::from(e));
    }
    // 3. Mint the API token with exactly the two bridge scopes. No
    // messages:write etc. so even without the role gate a leaked token
    // could only post bridge messages + heartbeat to owned bridges.
    let plaintext = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(secret, &plaintext);
    let scopes = format!(
        "{} {}",
        super::api::SCOPE_BRIDGE_POST,
        super::api::SCOPE_BRIDGE_HEARTBEAT
    );
    if let Err(e) =
        db::api_tokens::insert(&state.auth, &bot_id, "bridge token", &hash, &scopes, None).await
    {
        let _ = db::auth::delete_user(&state.auth, &bot_id).await;
        return Err(AppError::from(e));
    }
    // 4. Seal the daemon config and insert the bridges row.
    if let Err(e) = db::bridges::insert(
        &state.chat,
        secret,
        form.room_id,
        kind,
        config.as_bytes(),
        &bot_id,
        &actor.id,
    )
    .await
    {
        // Roll back the bot + its token so the admin can retry without a
        // stuck-bot ghost row.
        let _ = db::api_tokens::revoke_all_for_user(&state.auth, &bot_id).await;
        let _ = db::auth::delete_user(&state.auth, &bot_id).await;
        return Err(AppError::Internal(format!("bridge insert failed: {e}")));
    }
    db::moderation::log_mod_action(
        &state.chat,
        "bridge_create",
        &bot_id,
        &actor.id,
        Some(bot_username),
        None,
        Some(kind),
    )
    .await?;
    Ok(render_bridges_page(
        &state,
        &actor,
        Some(plaintext),
        Some(bot_username.to_string()),
        None,
    )
    .await?
    .into_response())
}

/// POST /admin/bridges/{id}/remove - stop-new lifecycle removal. Deletes
/// the bridges row, which triggers ON DELETE SET NULL on
/// `messages.bridge_id`; the snapshotted `bridge_foreign_name` +
/// `bridge_kind` columns persist so historical bridged messages still
/// render as "alice (via matrix)". Does NOT ban the bot or revoke its
/// token: the token's scopes are bridge:post + bridge:heartbeat, which
/// have nothing to act on once the bridge is gone. The admin can disable
/// the orphan bot via /admin/bots if they want.
pub async fn post_bridge_remove(
    State(state): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    let removed = db::bridges::remove(&state.chat, id).await?;
    if removed {
        db::moderation::log_mod_action(
            &state.chat,
            "bridge_remove",
            &id.to_string(),
            &actor.id,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(Redirect::to("/admin/bridges"))
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
    // SSRF guard: reject loopback / private / link-local / metadata targets
    // (reuses the LC-72 custom-command guard) so a webhook can't be pointed
    // at internal services.
    if !super::slash::webhook_url_ok(url) {
        return err(
            &state,
            &actor,
            "URL must be a public http(s) URL (no localhost or private IPs).",
        )
        .await;
    }
    // LC-357: validate the scoped target exists so a forged scope_id cannot
    // create an orphan webhook pointing at a nonexistent room/enclave.
    let scope_kind = form.scope_kind.as_str();
    let scope_id = match scope_kind {
        "global" => None,
        "room" => match form.scope_id {
            None => return err(&state, &actor, "Room scope needs a room id.").await,
            Some(id) if db::chat::get_room(&state.chat, id).await?.is_some() => Some(id),
            Some(_) => return err(&state, &actor, "Selected room does not exist.").await,
        },
        "enclave" => match form.scope_id {
            None => return err(&state, &actor, "Enclave scope needs an enclave id.").await,
            Some(id) if db::enclave::get_enclave(&state.chat, id).await?.is_some() => Some(id),
            Some(_) => return err(&state, &actor, "Selected enclave does not exist.").await,
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

/// LC-207: host for the at-a-glance failures cell, or the `<unparseable>`
/// sentinel when the stored `foreign_url` does not parse / has no host. The
/// sentinel flags the row as broken-upstream (a truncated raw string would
/// visually read like a host and hide the anomaly); the full raw value still
/// rides along in the row's `title=` tooltip for diagnosis.
fn host_or_sentinel(foreign_url: &str) -> String {
    url::Url::parse(foreign_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "<unparseable>".to_string())
}

/// LC-207: read-only bridge-avatar cache diagnostic page. Answers "why is
/// this bridged user showing initials" without dropping to SQL: a stats
/// header (counts by status, bytes on disk, oldest last-seen, stale-pending
/// anomaly counter) plus the last 50 failed fetches with failure_reason and
/// foreign host. Hybrid mirror of the deliveries page (table / empty-state /
/// read-only / static) rendered as a global nav-reachable page (the cache is
/// keyed by hash, owned by no bridge).
pub async fn get_bridge_avatars(
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

    let raw = db::bridge_avatar_proxies::cache_stats(&state.chat).await?;
    let stats = BridgeAvatarStatsView {
        total: raw.total,
        ok: raw.ok,
        pending: raw.pending,
        failed: raw.failed,
        pending_stale: raw.pending_stale,
        total_bytes_display: format_bytes_mib(raw.total_bytes),
        oldest_last_seen: raw.oldest_last_seen,
    };

    let failures: Vec<BridgeAvatarFailureView> =
        db::bridge_avatar_proxies::recent_failures(&state.chat, 50)
            .await?
            .into_iter()
            .map(|r| BridgeAvatarFailureView {
                foreign_host: host_or_sentinel(&r.foreign_url),
                foreign_url: r.foreign_url,
                failure_reason: r.failure_reason.unwrap_or_else(|| "-".to_string()),
                content_type: r.content_type,
                last_seen_at: r.last_seen_at,
            })
            .collect();

    html(&BridgeAvatarsPage {
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
        section: "bridges",
        stats: &stats,
        failures: &failures,
    })
}
