//! LC-83: per-enclave user group admin API.
//!
//! Mutations require enclave admin / owner / site admin via
//! `enclave_can_manage`. The mention parser
//! (`resolve_tokens_for_room`) consumes these groups when expanding
//! `@group-name` tokens; no UI is bundled here yet, manage via curl /
//! direct API until a templates pass lands.
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::perms::enclave_can_manage;
use crate::state::AppState;
use crate::views::enclave::{
    GroupMemberCandidate, GroupMemberCandidateState, GroupMemberRowResult,
    GroupMemberSearchFragment,
};
use crate::views::{html, Html};

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
///
/// Returns a row fragment (`GroupMemberRowResult`) the typeahead swaps
/// in via `hx-swap="outerHTML"`. Not a redirect because the caller is
/// the popover row, not a full-page form.
pub async fn post_add_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id)): Path<(i64, i64)>,
    Form(form): Form<MemberForm>,
) -> Result<Html, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    assert_group_in_enclave(&state, enclave_id, group_id).await?;
    // The target must be a member of the enclave (you can't add a
    // non-member to a group inside that enclave).
    let m = db::enclave::get_membership(&state.chat, enclave_id, &form.user_id).await?;
    if m.is_none() {
        return html(&GroupMemberRowResult {
            ok: false,
            message: "Not a member of this enclave.",
        });
    }
    db::user_groups::add_member(&state.chat, group_id, &form.user_id).await?;
    html(&GroupMemberRowResult {
        ok: true,
        message: "Added",
    })
}

#[derive(Deserialize)]
pub struct MemberSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
}

/// GET /enclave/{id}/groups/{group_id}/members/search?q=...
///
/// Typeahead candidates for adding a member to a group. Blank `q`
/// returns an empty body so the popover collapses via `empty:hidden`,
/// matching `/enclave/{id}/invite/search`.
pub async fn get_member_search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((enclave_id, group_id)): Path<(i64, i64)>,
    Query(MemberSearchQuery { q }): Query<MemberSearchQuery>,
) -> Result<Html, AppError> {
    require_manage(&state, &user, enclave_id).await?;
    assert_group_in_enclave(&state, enclave_id, group_id).await?;
    let query = q.unwrap_or_default();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(crate::views::Html(String::new()));
    }

    let records = db::auth::search_users(&state.auth, trimmed, &user.id, 50).await?;
    let enclave_members = db::enclave::list_members(&state.chat, enclave_id).await?;
    let enclave_ids: std::collections::HashSet<String> =
        enclave_members.into_iter().map(|m| m.user_id).collect();
    let group_ids: std::collections::HashSet<String> =
        db::user_groups::list_member_ids(&state.chat, group_id)
            .await?
            .into_iter()
            .collect();

    let results: Vec<GroupMemberCandidate> = records
        .into_iter()
        .map(|r| {
            let state_flag = if group_ids.contains(&r.id) {
                GroupMemberCandidateState::AlreadyInGroup
            } else if !enclave_ids.contains(&r.id) {
                GroupMemberCandidateState::NotInEnclave
            } else {
                GroupMemberCandidateState::Addable
            };
            GroupMemberCandidate {
                id: r.id,
                username: r.username,
                display_name: r.display_name,
                avatar_ext: r.avatar_ext,
                status: r.status,
                custom_status: r.custom_status,
                state: state_flag,
            }
        })
        .collect();

    html(&GroupMemberSearchFragment {
        enclave_id,
        group_id,
        query: trimmed,
        results: &results,
    })
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
