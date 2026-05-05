use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::enclave::EnclaveRole;
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::state::AppState;
use crate::perms::{enclave_can_delete, enclave_can_manage_admins};
use crate::views::enclave::{DiscoverPage, EnclavePage, EnclaveSettingsPage};
use crate::views::{html, Html};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enclaves", post(post_create))
        .route("/enclave/{id}", get(get_landing))
        .route("/enclaves/discover", get(get_discover))
        .route("/enclaves/discover/{id}/join", post(post_discover_join))
        .route("/enclaves/join", post(post_join_by_code))
        .route("/enclave/{id}/visibility", post(post_visibility))
        .route(
            "/enclave/{id}/invite-code",
            post(post_invite_code).delete(delete_invite_code),
        )
        .route("/enclave/{id}/invite", post(post_invite))
        .route("/invitations", get(get_invitations))
        .route("/invitations/{id}/accept", post(post_invitation_accept))
        .route("/invitations/{id}/decline", post(post_invitation_decline))
        .route("/enclave/{id}/settings", get(get_settings))
        .route("/enclave/{id}/edit", post(post_edit))
        .route("/enclave/{id}/transfer", post(post_transfer))
        .route("/enclave/{id}/delete", post(post_delete))
        .route("/enclave/{id}/leave", post(post_leave))
        .route("/enclave/{id}/members/{user_id}/role", post(post_member_role))
        .route("/enclave/{id}/members/{user_id}/kick", post(post_kick))
}

async fn require_manage(
    state: &AppState,
    user: &User,
    enclave_id: i64,
) -> Result<(), AppError> {
    let m = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    if !enclave_can_manage(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub name: String,
    pub description: Option<String>,
}

pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<impl IntoResponse, AppError> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let id = db::enclave::create_enclave(
        &state.chat,
        name,
        form.description.as_deref().filter(|s| !s.is_empty()),
        &user.id,
    )
    .await?;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn get_landing(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    let membership = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    let role = membership.as_ref().map(|m| m.role);
    let is_site_admin = user.role == "admin";
    if role.is_none() && !is_site_admin {
        return Err(AppError::Forbidden);
    }
    let can_manage = enclave_can_manage(role, &user.role);
    let members = db::enclave::list_members(&state.chat, id).await?;
    let rooms = db::chat::list_rooms_in_enclave(&state.chat, id, &user.id, can_manage).await?;
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    html(&EnclavePage {
        user: &user,
        enclave: &enclave,
        members: &members,
        rooms: &rooms,
        can_manage,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
    })
}

pub async fn get_discover(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let enclaves = db::enclave::list_public_enclaves(&state.chat).await?;
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    html(&DiscoverPage {
        user: &user,
        enclaves: &enclaves,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
    })
}

#[derive(Deserialize)]
pub struct VisibilityForm {
    pub is_public: String,
}

pub async fn post_visibility(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<VisibilityForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::set_public(&state.chat, id, form.is_public == "1").await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_invite_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let code: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    db::enclave::regenerate_invite_code(&state.chat, id, &code).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn delete_invite_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    db::enclave::clear_invite_code(&state.chat, id).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_discover_join(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if !enclave.is_public {
        return Err(AppError::Forbidden);
    }
    db::enclave::add_member(&state.chat, id, &user.id, EnclaveRole::Member).await?;
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

#[derive(Deserialize)]
pub struct JoinByCodeForm {
    pub code: String,
}

pub async fn post_join_by_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<JoinByCodeForm>,
) -> Result<impl IntoResponse, AppError> {
    let Some(enclave) =
        db::enclave::get_enclave_by_invite_code(&state.chat, form.code.trim()).await?
    else {
        return Err(AppError::BadRequest("invalid or revoked code".into()));
    };
    db::enclave::add_member(&state.chat, enclave.id, &user.id, EnclaveRole::Member).await?;
    Ok(Redirect::to(&format!("/enclave/{}", enclave.id)))
}

#[derive(Deserialize)]
pub struct InviteForm {
    pub username: String,
}

pub async fn post_invite(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<InviteForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let Some(target) = db::auth::find_user_by_username(&state.auth, form.username.trim()).await?
    else {
        return Err(AppError::BadRequest("user not found".into()));
    };
    if db::enclave::get_membership(&state.chat, id, &target.id)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest("user is already a member".into()));
    }
    if let Err(e) = db::enclave::create_invitation(&state.chat, id, &target.id, &user.id).await {
        if !matches!(&e, sqlx::Error::Database(d) if d.is_unique_violation()) {
            return Err(e.into());
        }
    }
    Ok(Redirect::to(&format!("/enclave/{id}")))
}

pub async fn post_invitation_accept(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(inv) = db::enclave::get_invitation(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if inv.invitee_id != user.id {
        return Err(AppError::Forbidden);
    }
    let (eid, _) = db::enclave::accept_invitation(&state.chat, id).await?;
    Ok(Redirect::to(&format!("/enclave/{eid}")))
}

pub async fn post_invitation_decline(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(inv) = db::enclave::get_invitation(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    if inv.invitee_id != user.id {
        return Err(AppError::Forbidden);
    }
    db::enclave::delete_invitation(&state.chat, id).await?;
    Ok(Redirect::to("/invitations"))
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    let Some(enclave) = db::enclave::get_enclave(&state.chat, id).await? else {
        return Err(AppError::NotFound);
    };
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    let role = m.as_ref().map(|x| x.role);
    if !enclave_can_manage(role, &user.role) {
        return Err(AppError::Forbidden);
    }
    let can_delete = enclave_can_delete(role, &user.role);
    let members = db::enclave::list_members(&state.chat, id).await?;
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    html(&EnclaveSettingsPage {
        user: &user,
        enclave: &enclave,
        members: &members,
        can_delete,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
    })
}

#[derive(Deserialize)]
pub struct EditForm {
    pub name: String,
    pub description: Option<String>,
}

pub async fn post_edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<EditForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    db::enclave::update_metadata(
        &state.chat,
        id,
        name,
        form.description.as_deref().filter(|s| !s.is_empty()),
    )
    .await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

#[derive(Deserialize)]
pub struct TransferForm {
    pub new_owner_id: String,
}

pub async fn post_transfer(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<TransferForm>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_manage_admins(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    db::enclave::transfer_ownership(&state.chat, id, form.new_owner_id.trim()).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_delete(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    db::enclave::delete_enclave(&state.chat, id).await?;
    Ok(Redirect::to("/"))
}

pub async fn post_leave(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let Some(m) = db::enclave::get_membership(&state.chat, id, &user.id).await? else {
        return Err(AppError::NotFound);
    };
    if matches!(m.role, EnclaveRole::Owner) {
        let members = db::enclave::list_members(&state.chat, id).await?;
        if members.len() == 1 {
            return Err(AppError::BadRequest(
                "delete the enclave instead of leaving".into(),
            ));
        }
        return Err(AppError::BadRequest(
            "transfer ownership before leaving".into(),
        ));
    }
    db::enclave::remove_member(&state.chat, id, &user.id).await?;
    Ok(Redirect::to("/"))
}

#[derive(Deserialize)]
pub struct RoleForm {
    pub role: String,
}

pub async fn post_member_role(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, target)): Path<(i64, String)>,
    axum::Form(form): axum::Form<RoleForm>,
) -> Result<impl IntoResponse, AppError> {
    let m = db::enclave::get_membership(&state.chat, id, &user.id).await?;
    if !enclave_can_manage_admins(m.map(|x| x.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    let new_role = match form.role.as_str() {
        "admin" => EnclaveRole::Admin,
        "member" => EnclaveRole::Member,
        _ => return Err(AppError::BadRequest("invalid role".into())),
    };
    db::enclave::update_role(&state.chat, id, &target, new_role).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn post_kick(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, target)): Path<(i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, id).await?;
    let Some(target_m) = db::enclave::get_membership(&state.chat, id, &target).await? else {
        return Err(AppError::NotFound);
    };
    if matches!(target_m.role, EnclaveRole::Owner) {
        return Err(AppError::BadRequest(
            "cannot kick the owner; transfer ownership first".into(),
        ));
    }
    db::enclave::remove_member(&state.chat, id, &target).await?;
    Ok(Redirect::to(&format!("/enclave/{id}/settings")))
}

pub async fn get_invitations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let invs = db::enclave::list_invitations_for_user(&state.chat, &user.id).await?;
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    html(&crate::views::enclave::InvitationsPage {
        user: &user,
        invitations: &invs,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
    })
}
