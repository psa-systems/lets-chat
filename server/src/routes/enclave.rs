use axum::extract::State;
use axum::response::{IntoResponse, Redirect};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/enclaves", post(post_create))
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
