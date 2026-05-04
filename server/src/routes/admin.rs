use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

use crate::auth::AdminUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::admin::{
    AdminInviteView, AdminRoomView, AdminUserView, InvitesPage, ModLogPage, RoomRowFragment,
    RoomsPage, SettingsPage, UserRowFragment, UsersPage,
};
use crate::views::{html, Html};
use crate::ws::events::ChatEvent;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin", get(get_settings))
        .route("/admin/settings", get(get_settings).post(post_settings))
        .route("/admin/users", get(get_users))
        .route("/admin/users/{id}/ban", post(post_ban))
        .route("/admin/users/{id}/unban", post(post_unban))
        .route("/admin/users/{id}/mute", post(post_mute))
        .route("/admin/users/{id}/unmute", post(post_unmute))
        .route("/admin/users/{id}/role", post(post_role))
        .route("/admin/users/{id}/delete", post(post_delete_user))
        .route("/admin/invites", get(get_invites).post(post_create_invite))
        .route("/admin/invites/{id}/revoke", post(post_revoke_invite))
        .route("/admin/rooms", get(get_rooms).post(post_create_room))
        .route("/admin/rooms/{id}/archive", post(post_archive_room))
        .route("/admin/rooms/{id}/edit", post(post_edit_room))
        .route("/admin/rooms/{id}/invite", post(post_invite_to_room))
        .route("/admin/rooms/{id}/regenerate", post(post_regenerate_invite))
        .route("/admin/modlog", get(get_modlog))
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

pub async fn get_settings(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
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
    let page = SettingsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
        section: "settings",
        smtp_host,
        smtp_port,
        smtp_user,
        smtp_from,
        saved: false,
    };
    html(&page)
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

// Users ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: String,
}

pub async fn get_users(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
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
        asset_version: &state.asset_version,
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
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    let invites = build_invite_views(&state).await?;
    let page = InvitesPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
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
pub struct CreateRoomForm {
    pub name: String,
    #[serde(default)]
    pub topic: String,
    pub room_type: String,
}

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
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
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
        asset_version: &state.asset_version,
        section: "rooms",
        rooms_admin: &rooms_admin,
    };
    html(&page)
}

pub async fn post_create_room(
    State(state): State<AppState>,
    AdminUser(_actor): AdminUser,
    axum::Form(form): axum::Form<CreateRoomForm>,
) -> Result<Response, AppError> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let room_type = match form.room_type.as_str() {
        "public" | "private" => form.room_type.as_str(),
        _ => return Err(AppError::BadRequest("invalid room_type".into())),
    };
    let topic = if form.topic.trim().is_empty() {
        None
    } else {
        Some(form.topic.trim())
    };
    let invite_code = if room_type == "private" {
        Some(random_code(10))
    } else {
        None
    };
    db::chat::create_room(&state.chat, name, topic, room_type, invite_code.as_deref()).await?;
    Ok(Redirect::to("/admin/rooms").into_response())
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
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    let entries = db::moderation::list_mod_actions(&state.chat).await?;
    let page = ModLogPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
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
