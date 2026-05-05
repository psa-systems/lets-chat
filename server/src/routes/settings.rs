use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::settings::UserSettingsPage;
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    pub read_receipts_enabled: Option<String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let page = UserSettingsPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        saved: false,
    };
    html(&page)
}

pub async fn post_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SettingsForm>,
) -> Result<Response, AppError> {
    let enabled = form.read_receipts_enabled.is_some();
    db::auth::set_read_receipts_enabled(&state.auth, &user.id, enabled).await?;
    Ok(Redirect::to("/settings").into_response())
}
