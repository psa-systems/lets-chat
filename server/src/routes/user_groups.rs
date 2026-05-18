//! LC-83: per-enclave user group admin API.
//!
//! Mutations require enclave admin / owner / site admin via
//! `enclave_can_manage`. The mention parser
//! (`resolve_tokens_for_room`) consumes these groups when expanding
//! `@group-name` tokens; no UI is bundled here yet, manage via curl /
//! direct API until a templates pass lands.
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct GroupForm {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct MemberForm {
    pub user_id: String,
}

async fn require_manage(state: &AppState, user: &User, enclave_id: i64) -> Result<(), AppError> {
    let membership = db::enclave::get_membership(&state.chat, enclave_id, &user.id).await?;
    if !enclave_can_manage(membership.map(|m| m.role), &user.role) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// POST /enclave/{id}/groups
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(enclave_id): Path<i64>,
    Form(form): Form<GroupForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("group name cannot be empty".into()));
    }
    db::user_groups::create(
        &state.chat,
        enclave_id,
        name,
        form.description.as_deref().filter(|s| !s.is_empty()),
        &user.id,
    )
    .await?;
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")))
}

/// PATCH /enclave/{id}/groups/{group_id}
pub async fn patch_rename(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id)): Path<(i64, i64)>,
    Form(form): Form<GroupForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("group name cannot be empty".into()));
    }
    let n = db::user_groups::rename(&state.chat, enclave_id, group_id, name).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")))
}

/// DELETE /enclave/{id}/groups/{group_id}
pub async fn delete_group(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    let n = db::user_groups::delete(&state.chat, enclave_id, group_id).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")))
}

/// POST /enclave/{id}/groups/{group_id}/members  body: user_id=...
pub async fn post_add_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id)): Path<(i64, i64)>,
    Form(form): Form<MemberForm>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    assert_group_in_enclave(&state, enclave_id, group_id).await?;
    // The target must be a member of the enclave (you can't add a
    // non-member to a group inside that enclave).
    let m = db::enclave::get_membership(&state.chat, enclave_id, &form.user_id).await?;
    if m.is_none() {
        return Err(AppError::BadRequest(
            "target user is not a member of this enclave".into(),
        ));
    }
    db::user_groups::add_member(&state.chat, group_id, &form.user_id).await?;
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")))
}

/// DELETE /enclave/{id}/groups/{group_id}/members/{user_id}
pub async fn delete_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id, target)): Path<(i64, i64, String)>,
) -> Result<impl IntoResponse, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    assert_group_in_enclave(&state, enclave_id, group_id).await?;
    db::user_groups::remove_member(&state.chat, group_id, &target).await?;
    Ok(Redirect::to(&format!("/enclave/{enclave_id}/settings")))
}

async fn assert_group_in_enclave(
    state: &AppState,
    enclave_id: i64,
    group_id: i64,
) -> Result<(), AppError> {
    let groups = db::user_groups::list_for_enclave(&state.chat, enclave_id).await?;
    if !groups.iter().any(|g| g.id == group_id) {
        return Err(AppError::NotFound);
    }
    Ok(())
}
