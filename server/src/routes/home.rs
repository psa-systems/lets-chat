use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::last_visited;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::html;

pub async fn get_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(path) = last_visited::read(&headers) {
        if last_visited::is_safe_path(&path) && target_accessible(&state, &user, &path).await? {
            return Ok(Redirect::to(&path).into_response());
        }
    }
    let (sidebar_rooms, sidebar_peers) = super::load_sidebar(&state, &user).await?;
    let page = WelcomePage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        asset_version: &state.asset_version,
    };
    let body = html(&page)?;
    Ok(body.into_response())
}

async fn target_accessible(state: &AppState, user: &User, path: &str) -> Result<bool, AppError> {
    if let Some(rest) = path.strip_prefix("/room/") {
        let id: i64 = match rest.parse() {
            Ok(n) => n,
            Err(_) => return Ok(false),
        };
        return Ok(crate::db::chat::is_room_accessible(
            &state.chat,
            id,
            &user.id,
            user.role == "admin",
        )
        .await?);
    }
    if let Some(peer_id) = path.strip_prefix("/dm/") {
        if peer_id == user.id {
            return Ok(false);
        }
        let other = crate::db::auth::find_user_by_id(&state.auth, peer_id).await?;
        return Ok(other.is_some());
    }
    Ok(false)
}
