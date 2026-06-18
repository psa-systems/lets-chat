//! LC-342: shame-tag voting + moderator override endpoints. All gated on the
//! message's enclave having the prototype enabled. Voting needs room access;
//! the override needs room-manage. Each handler returns the re-rendered control
//! fragment so the popover updates in place.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use std::collections::HashSet;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::perms::room_can_manage_overrides;
use crate::state::AppState;
use crate::views::shame_tags::{ShameTagControl, ShameTagRow};
use crate::views::{html, Html};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/messages/{id}/tags", get(get_control))
        .route("/messages/{id}/tags/{tag}", post(post_vote))
        .route("/messages/{id}/tag-override", post(post_override))
}

/// Resolve the message's room and confirm shame-tagging is on for its enclave.
/// Returns the room id, or NotFound when the message/feature is absent.
async fn room_for_enabled_message(state: &AppState, message_id: i64) -> Result<i64, AppError> {
    let room_id = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?
        .room_id;
    if !db::enclave::shame_tags_enabled_for_room(&state.chat, room_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(room_id)
}

async fn build_control(
    state: &AppState,
    message_id: i64,
    room_id: i64,
    user: &User,
) -> Result<ShameTagControl, AppError> {
    let counts = db::shame_tags::tag_counts(&state.chat, message_id).await?;
    let voted: HashSet<String> = db::shame_tags::voter_tags(&state.chat, message_id, &user.id)
        .await?
        .into_iter()
        .collect();
    let tags = db::shame_tags::TAGS
        .iter()
        .map(|t| ShameTagRow {
            tag: (*t).to_string(),
            count: counts.get(*t).copied().unwrap_or(0),
            voted: voted.contains(*t),
        })
        .collect();
    let enclave_role = match super::enclave_for_room(state, room_id).await? {
        Some(eid) => db::enclave::get_membership(&state.chat, eid, &user.id)
            .await?
            .map(|m| m.role),
        None => None,
    };
    let can_manage = room_can_manage_overrides(enclave_role, &user.role);
    let override_hidden = db::shame_tags::get_override(&state.chat, message_id).await?;
    Ok(ShameTagControl {
        message_id,
        tags,
        can_manage,
        override_hidden,
    })
}

/// GET the control (lazy-loaded into the hover menu on open). Room access only.
pub async fn get_control(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Html, AppError> {
    let room_id = room_for_enabled_message(&state, id).await?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    html(&build_control(&state, id, room_id, &user).await?)
}

/// Toggle the caller's vote on a tag, then return the refreshed control.
pub async fn post_vote(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, tag)): Path<(i64, String)>,
) -> Result<Html, AppError> {
    let room_id = room_for_enabled_message(&state, id).await?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    if !db::shame_tags::is_valid_tag(&tag) {
        return Err(AppError::BadRequest("unknown tag".into()));
    }
    db::shame_tags::toggle_vote(&state.chat, id, &tag, &user.id).await?;
    html(&build_control(&state, id, room_id, &user).await?)
}

#[derive(Deserialize)]
pub struct OverrideForm {
    /// "hide" | "show" | "clear".
    pub state: String,
}

/// Moderator override (force hide / show / clear), then return the control.
pub async fn post_override(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<OverrideForm>,
) -> Result<Html, AppError> {
    let room_id = room_for_enabled_message(&state, id).await?;
    super::room_rbac::require_can_manage(&state, &user, room_id).await?;
    match form.state.as_str() {
        "hide" => db::shame_tags::set_override(&state.chat, id, true, &user.id).await?,
        "show" => db::shame_tags::set_override(&state.chat, id, false, &user.id).await?,
        "clear" => db::shame_tags::clear_override(&state.chat, id).await?,
        _ => return Err(AppError::BadRequest("bad override state".into())),
    }
    html(&build_control(&state, id, room_id, &user).await?)
}
