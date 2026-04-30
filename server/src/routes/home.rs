use axum::extract::State;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::{html, Html};

pub async fn get_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    let rooms = db::chat::list_rooms(&state.chat, &user.id, is_admin).await?;

    // DM peers: list the user's DM rooms (each pairs the user with one peer
    // user_id), then resolve each peer_id to a public `User` from the auth DB.
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peers: Vec<User> = Vec::with_capacity(dm_rooms.len());
    for (_room, peer_id) in &dm_rooms {
        if let Some(record) = db::auth::find_user_by_id(&state.auth, peer_id).await? {
            dm_peers.push(record.into());
        }
    }

    let page = WelcomePage {
        user: &user,
        rooms: &rooms,
        dm_peers: &dm_peers,
        asset_version: state.asset_version,
    };
    html(&page)
}
