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
    let page = WelcomePage {
        user: &user,
        asset_version: state.asset_version,
    };
    html(&page)
}
