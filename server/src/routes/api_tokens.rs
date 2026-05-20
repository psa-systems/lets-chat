//! LC-72: settings UI to mint / list / revoke personal API tokens. These
//! are cookie-authenticated settings pages (not part of the bearer API).

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::api_tokens::{ApiTokenRowView, ApiTokensPage};
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct CreateForm {
    pub name: String,
    /// Checkboxes, one per scope (value "on" when checked).
    #[serde(default)]
    pub s_messages_read: Option<String>,
    #[serde(default)]
    pub s_messages_write: Option<String>,
    #[serde(default)]
    pub s_rooms_read: Option<String>,
    /// Optional lifetime in days; blank / 0 = never expires.
    #[serde(default)]
    pub expires_days: Option<i64>,
}

async fn render(
    state: &AppState,
    user: &crate::models::User,
    new_token: Option<String>,
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
    let tokens: Vec<ApiTokenRowView> = db::api_tokens::list_for_user(&state.auth, &user.id)
        .await?
        .into_iter()
        .map(|t| ApiTokenRowView {
            id: t.id,
            name: t.name,
            scopes: t.scopes,
            created_at: t.created_at,
            last_used_at: t.last_used_at,
            expires_at: t.expires_at,
            revoked: t.revoked_at.is_some(),
        })
        .collect();
    let page = ApiTokensPage {
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
        available: state.secret_key.is_some(),
        all_scopes: super::api::ALL_SCOPES,
        tokens: &tokens,
        new_token,
        error,
    };
    html(&page)
}

/// GET /settings/api-tokens
pub async fn get_api_tokens(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    render(&state, &user, None, None).await
}

/// POST /settings/api-tokens
pub async fn post_api_tokens(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<Response, AppError> {
    let Some(secret) = state.secret_key.as_ref() else {
        return Ok(render(
            &state,
            &user,
            None,
            Some("API tokens are unavailable: the server has no secret key configured.".into()),
        )
        .await?
        .into_response());
    };

    let name = form.name.trim();
    if name.is_empty() {
        return Ok(
            render(&state, &user, None, Some("Token name is required.".into()))
                .await?
                .into_response(),
        );
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
        return Ok(render(
            &state,
            &user,
            None,
            Some("Select at least one scope.".into()),
        )
        .await?
        .into_response());
    }
    let scopes = scopes.join(" ");

    let expires_at = match form.expires_days {
        Some(d) if d > 0 => Some(
            (Utc::now() + Duration::days(d))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        ),
        _ => None,
    };

    let plaintext = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(secret, &plaintext);
    db::api_tokens::insert(
        &state.auth,
        &user.id,
        name,
        &hash,
        &scopes,
        expires_at.as_deref(),
    )
    .await?;

    // Show the plaintext exactly once.
    Ok(render(&state, &user, Some(plaintext), None)
        .await?
        .into_response())
}

/// POST /settings/api-tokens/{id}/revoke
pub async fn post_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    db::api_tokens::revoke(&state.auth, id, &user.id).await?;
    Ok(Redirect::to("/settings/api-tokens"))
}
