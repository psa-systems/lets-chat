use axum::extract::State;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::{html, Html};

pub async fn get_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;

    let page = WelcomePage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: state.asset_version,
    };
    html(&page)
}
