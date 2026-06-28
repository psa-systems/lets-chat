//! LC-487: per-user canned responses / saved replies. Managed from the user's
//! Settings page; invoked from the composer as a personal slash command
//! (`/name [args]`) and expanded by `routes::slash::try_dispatch`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// Cap on a canned-response description (single help line).
const MAX_DESC_CHARS: usize = 200;

#[derive(Deserialize)]
pub struct CannedForm {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
}

/// A name must be a slash-command token: lowercase letters, digits, `-`, `_`.
/// Mirrors the admin custom-command rule (`routes::admin::post_slash_commands`).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// POST /settings/canned - create a canned response for the caller.
pub async fn post_create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<CannedForm>,
) -> Result<Response, AppError> {
    let name = form
        .name
        .trim()
        .trim_start_matches('/')
        .to_ascii_lowercase();
    if !valid_name(&name) {
        return Err(AppError::BadRequest(
            "name must be non-empty letters, digits, - or _".into(),
        ));
    }
    // A canned response cannot shadow a built-in command (built-ins win at
    // dispatch, so it would never fire).
    if crate::commands::find_builtin(&name).is_some() {
        return Err(AppError::BadRequest(
            "that name is a built-in command".into(),
        ));
    }
    let body = form.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("body is required".into()));
    }
    // Cap the body at the message length limit: it is posted as a message.
    let body: String = body
        .chars()
        .take(crate::routes::room::MAX_MESSAGE_CHARS)
        .collect();
    let description: String = form
        .description
        .trim()
        .chars()
        .take(MAX_DESC_CHARS)
        .collect();

    match db::slash::insert_canned(&state.chat, &user.id, &name, &description, &body).await {
        Ok(_) => {}
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            return Ok((
                StatusCode::CONFLICT,
                format!("you already have a saved reply named /{name}"),
            )
                .into_response());
        }
        Err(e) => return Err(e.into()),
    }
    Ok(Redirect::to("/settings?ok=canned-added#canned").into_response())
}

/// POST /settings/canned/{id}/delete - remove one of the caller's canned
/// responses. Owner-scoped; 404 if it is not theirs.
pub async fn post_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let removed = db::slash::delete_canned(&state.chat, &user.id, id).await?;
    if removed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/settings?ok=canned-deleted#canned").into_response())
}
