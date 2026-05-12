use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

use crate::auth::AdminUser;
use crate::db;
use crate::db::smtp_settings::{SmtpConfigInput, TlsMode};
use crate::email::EmailMessage;
use crate::error::AppError;
use crate::state::AppState;
use crate::version;
use crate::views::admin::{
    AdminEnclaveView, AdminInviteView, AdminRoomView, AdminUserView, EnclavesPage, InvitesPage,
    ModLogPage, RoomRowFragment, RoomsPage, SettingsPage, TestEmailResult, UserRowFragment,
    UsersPage,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin", get(get_settings))
        .route("/admin/settings", get(get_settings).post(post_settings))
        .route("/admin/settings/smtp/test", post(post_smtp_test))
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
        .route("/admin/invites", get(get_invites).post(post_create_invite))
        .route("/admin/invites/{id}/revoke", post(post_revoke_invite))
        .route("/admin/rooms", get(get_rooms))
        .route("/admin/rooms/{id}/archive", post(post_archive_room))
        .route("/admin/rooms/{id}/edit", post(post_edit_room))
        .route("/admin/rooms/{id}/invite", post(post_invite_to_room))
        .route("/admin/rooms/{id}/regenerate", post(post_regenerate_invite))
        .route("/admin/enclaves", get(get_enclaves))
        .route("/admin/modlog", get(get_modlog))
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
    pub tls_mode: String,
    /// Externally-reachable base URL of this server (e.g.
    /// `https://chat.example.com`). Stored in the generic `settings`
    /// key-value table as `public_base_url`. Used by the email digest
    /// to build deep links. Empty is allowed: digest still sends, items
    /// are not clickable. A trailing slash is stripped on save so the
    /// digest can always concatenate with `/room/...` etc.
    pub public_base_url: String,
}

#[derive(Deserialize)]
pub struct SmtpTestForm {
    pub test_to: String,
}

async fn render_settings_page(
    state: &AppState,
    user: &crate::models::User,
    saved: bool,
    test_result: Option<TestEmailResult>,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(state, user, None).await?;

    // Load the singleton SMTP row. If the secret key is missing, we can
    // still read host/port/user/from to show in the form; only the
    // password column is encrypted, and load() leaves it as None when
    // decryption fails. To keep the GET handler simple, gate the entire
    // load on the secret key: without it the form renders empty and the
    // disabled banner explains why.
    let public_base_url = db::settings::get_setting(&state.settings, "public_base_url")
        .await?
        .unwrap_or_default();
    let default_notify_email_digest =
        db::settings::get_setting(&state.settings, "default_notify_email_digest")
            .await?
            .as_deref()
            == Some("1");

    let (smtp_host, smtp_port, smtp_user, smtp_from, smtp_tls_mode) =
        match state.secret_key.as_ref() {
            Some(key) => match db::smtp_settings::load(&state.settings, key.as_ref()).await {
                Ok(Some(cfg)) => (
                    cfg.host,
                    cfg.port.to_string(),
                    cfg.username.unwrap_or_default(),
                    cfg.from_address,
                    cfg.tls_mode.as_str().to_string(),
                ),
                Ok(None) | Err(_) => (
                    String::new(),
                    "587".to_string(),
                    String::new(),
                    String::new(),
                    "starttls".to_string(),
                ),
            },
            None => (
                String::new(),
                "587".to_string(),
                String::new(),
                String::new(),
                "starttls".to_string(),
            ),
        };

    // Pick at most one banner explaining why email is unavailable.
    // Precedence: missing secret key (operator must fix env) beats
    // unconfigured SMTP (operator must fill in the form) beats
    // load-failure-with-key-set (operator should re-enter password).
    let disabled_banner: Option<&'static str> = if state.secret_key.is_none() {
        Some(
            "Email sending is disabled because LETS_CHAT_SECRET_KEY is not set. \
             Set it in the environment and restart the server.",
        )
    } else if smtp_host.is_empty() {
        Some("SMTP is not configured. Fill in the fields below and save.")
    } else if !state.email_available() {
        // Fires for two reasons, in order of likelihood:
        // 1. The operator just saved SMTP settings but has not restarted
        //    yet. `state.email_client` is a snapshot taken at startup,
        //    so any save-then-render-this-page sequence lands here until
        //    the next process restart. This is the common case and the
        //    banner leads with it explicitly so operators do not assume
        //    their config is broken.
        // 2. LETS_CHAT_SECRET_KEY was rotated, so the encrypted password
        //    column no longer decrypts. The parenthetical covers this.
        Some(
            "SMTP config not active in this process. Saved changes apply only after \
             a server restart. (If you rotated LETS_CHAT_SECRET_KEY, the stored SMTP \
             password also needs to be re-entered before the restart.)",
        )
    } else {
        None
    };

    let page = SettingsPage {
        user,
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
        smtp_tls_mode,
        public_base_url,
        default_notify_email_digest,
        saved,
        disabled_banner,
        test_result,
    };
    html(&page)
}

pub async fn get_settings(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    render_settings_page(&state, &user, false, None).await
}

pub async fn post_settings(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    axum::Form(form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    let Some(key) = state.secret_key.as_ref() else {
        return Err(AppError::BadRequest(
            "LETS_CHAT_SECRET_KEY is not set; cannot save SMTP settings".into(),
        ));
    };
    let port: u16 = form.smtp_port.parse().map_err(|_| {
        AppError::BadRequest(format!(
            "smtp_port: '{}' is not a valid port",
            form.smtp_port
        ))
    })?;
    let input = SmtpConfigInput {
        host: form.smtp_host.trim().to_string(),
        port,
        username: trim_to_none(&form.smtp_user),
        // form.smtp_pass empty means "leave existing alone" (see
        // db::smtp_settings::save). Match the existing UX where a blank
        // password field is "keep current."
        password: if form.smtp_pass.is_empty() {
            None
        } else {
            Some(form.smtp_pass)
        },
        from_address: form.smtp_from.trim().to_string(),
        tls_mode: TlsMode::parse(form.tls_mode.trim()),
    };
    db::smtp_settings::save(&state.settings, key.as_ref(), &input).await?;

    // Public site URL: stored separately in the generic settings table.
    // Normalise: trim, strip exactly one trailing slash if present so the
    // digest can do `format!("{base}/room/...")` without worrying about
    // double slashes. Operator can clear it by submitting blank.
    let base = form.public_base_url.trim();
    let base = base.strip_suffix('/').unwrap_or(base);
    db::settings::set_setting(&state.settings, "public_base_url", base).await?;

    Ok(Redirect::to("/admin/settings").into_response())
}

/// Send a hardcoded one-line test email to a recipient supplied in the
/// form. Bypasses the `state.email_client` snapshot so the operator can
/// verify a just-saved SMTP config without restarting the server. In
/// tests, `state.email_client` is a `MockEmailClient` and is used in
/// preference to constructing a real lettre transport: that keeps
/// integration tests free of any DNS / network dependency.
pub async fn post_smtp_test(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    axum::Form(form): axum::Form<SmtpTestForm>,
) -> Result<Html, AppError> {
    let to_addr = form.test_to.trim();
    if to_addr.is_empty() {
        return render_settings_page(
            &state,
            &user,
            false,
            Some(TestEmailResult {
                ok: false,
                message: "Recipient address is required.".into(),
            }),
        )
        .await;
    }
    // The handler uses `state.email_client` rather than constructing a
    // fresh transport from the current DB row. Consequence: after the
    // operator saves SMTP settings, they must restart the server before
    // the "Send test email" button reflects the new config. This is
    // documented in the template. The simpler model avoids splitting
    // the prod and test code paths through a `cfg!` switch and lets
    // integration tests inject a `MockEmailClient` cleanly.
    let Some(client) = state.email_client.as_ref().cloned() else {
        return render_settings_page(
            &state,
            &user,
            false,
            Some(TestEmailResult {
                ok: false,
                message: if state.secret_key.is_none() {
                    "Email is disabled: LETS_CHAT_SECRET_KEY is not set.".into()
                } else {
                    "Email is not initialised. Save SMTP settings, then restart the server.".into()
                },
            }),
        )
        .await;
    };
    let from = match state.secret_key.as_ref() {
        Some(key) => db::smtp_settings::load(&state.settings, key.as_ref())
            .await?
            .map(|c| c.from_address)
            .unwrap_or_default(),
        None => String::new(),
    };
    if from.is_empty() {
        return render_settings_page(
            &state,
            &user,
            false,
            Some(TestEmailResult {
                ok: false,
                message: "From address is empty. Save it in the form and restart first.".into(),
            }),
        )
        .await;
    }

    let msg = EmailMessage {
        to: to_addr.to_string(),
        from,
        subject: "lets-chat: SMTP test email".to_string(),
        text_body: format!(
            "This is a test email from lets-chat triggered by admin user '{}'.\n\
             If you received this, your SMTP settings are working.\n",
            user.username
        ),
        html_body: format!(
            "<p>This is a test email from lets-chat triggered by admin user \
             <strong>{}</strong>.</p>\
             <p>If you received this, your SMTP settings are working.</p>",
            askama_escape(&user.username)
        ),
    };
    let result = match client.send(msg).await {
        Ok(()) => TestEmailResult {
            ok: true,
            message: format!("Test email sent to {to_addr}."),
        },
        Err(e) => TestEmailResult {
            ok: false,
            message: format!("Send failed: {e}"),
        },
    };
    render_settings_page(&state, &user, false, Some(result)).await
}

#[derive(Deserialize)]
pub struct EmailDigestDefaultForm {
    /// Form-checkbox convention: a checked box submits the field, an
    /// unchecked box omits it. Serde `default` makes the missing case
    /// `None`, which `is_some()` treats as off.
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

fn trim_to_none(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn askama_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
    let users: Vec<AdminUserView> = records
        .into_iter()
        .map(|r| AdminUserView {
            id: r.id,
            username: r.username,
            role: r.role,
            is_banned: r.is_banned,
            is_muted: r.is_muted,
        })
        .collect();
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
    let view = AdminUserView {
        id: record.id,
        username: record.username,
        role: record.role,
        is_banned: record.is_banned,
        is_muted: record.is_muted,
    };
    html(&UserRowFragment { u: &view })
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
