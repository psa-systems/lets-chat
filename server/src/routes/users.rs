use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::routes::effective_status;
use crate::state::AppState;
use crate::views::users::{ProfileResult, ProfileSearchFragment};
use crate::views::{html, Html};

const MAX_RESULTS: i64 = 50;

#[derive(Deserialize)]
pub struct UserSearchQuery {
    pub q: Option<String>,
}

/// GET /users/search?q=... - substring match against username and display_name.
///
/// Returns a popover fragment rendered into the sidebar's
/// `#search-people-results` target. An empty query returns an empty body so
/// the popover collapses via `empty:hidden`.
pub async fn get_user_search(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Query(UserSearchQuery { q }): Query<UserSearchQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return Ok(Html(String::new()));
    }

    let records = db::auth::search_users(&state.auth, trimmed, &viewer.id, MAX_RESULTS).await?;
    let results: Vec<ProfileResult> = records
        .into_iter()
        .map(|r| {
            let is_self = r.id == viewer.id;
            let status = if is_self {
                r.status.clone()
            } else {
                effective_status(&state, &r.id, &r.status)
            };
            ProfileResult {
                id: r.id,
                username: r.username,
                display_name: r.display_name,
                avatar_ext: r.avatar_ext,
                status,
                custom_status: r.custom_status,
                bio: r.bio,
                is_self,
            }
        })
        .collect();

    let fragment = ProfileSearchFragment {
        query: trimmed,
        results: &results,
    };
    html(&fragment)
}

#[derive(Deserialize)]
pub struct BlockReturn {
    #[serde(default)]
    pub return_to: Option<String>,
}

fn safe_return_to(value: Option<&str>) -> String {
    // Only honour same-origin paths. A return_to of "/" is fine; absolute
    // URLs and protocol-relative URLs are ignored to prevent open redirects.
    match value {
        Some(v) if v.starts_with('/') && !v.starts_with("//") => v.to_string(),
        _ => "/settings/blocked".to_string(),
    }
}

/// POST /users/{id}/block - block another user. Idempotent. Self-blocks
/// return 400 so the UI never has to render a "block yourself" button.
pub async fn post_block(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Path(target_id): Path<String>,
    axum::Form(form): axum::Form<BlockReturn>,
) -> Result<Response, AppError> {
    if target_id == viewer.id {
        return Err(AppError::BadRequest("cannot block yourself".into()));
    }
    db::auth::find_user_by_id(&state.auth, &target_id)
        .await?
        .ok_or(AppError::NotFound)?;
    db::auth::block_user(&state.auth, &viewer.id, &target_id).await?;
    Ok(Redirect::to(&safe_return_to(form.return_to.as_deref())).into_response())
}

/// POST /users/{id}/unblock - reverse of block. Idempotent.
pub async fn post_unblock(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Path(target_id): Path<String>,
    axum::Form(form): axum::Form<BlockReturn>,
) -> Result<Response, AppError> {
    db::auth::unblock_user(&state.auth, &viewer.id, &target_id).await?;
    Ok(Redirect::to(&safe_return_to(form.return_to.as_deref())).into_response())
}
