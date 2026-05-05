use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::perms::enclave_can_manage;
use crate::state::AppState;
use crate::views::enclave::EnclavePage;
use crate::views::{html, Html};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enclaves", post(post_create))
        .route("/enclave/{id}", get(get_landing))
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
