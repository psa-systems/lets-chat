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
use crate::views::enclave::{DiscoverPage, EnclavePage};
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
