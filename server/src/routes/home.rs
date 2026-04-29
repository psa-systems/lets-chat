use axum::extract::State;

use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::{html, Html};

pub async fn get_home(State(state): State<AppState>) -> Result<Html, AppError> {
    let user = User::placeholder();
    let page = WelcomePage {
        user: &user,
        asset_version: state.asset_version,
    };
    html(&page)
}
