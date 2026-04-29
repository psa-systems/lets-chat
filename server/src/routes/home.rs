use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::WelcomePage;

pub async fn get_home(State(state): State<AppState>) -> Result<Response, AppError> {
    let placeholder = User::placeholder();
    let page = WelcomePage {
        user: &placeholder,
        asset_version: state.asset_version,
    };
    let body = page.render()?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response())
}
