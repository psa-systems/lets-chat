use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::last_visited;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::html;

#[derive(Deserialize)]
pub struct HomeQuery {
    /// When `home=1`, render the Home pseudo-enclave directly and skip the
    /// `last_visited` redirect. The switcher's Home button uses this so that
    /// the user can explicitly go back to the DM hub from any room.
    #[serde(default)]
    pub home: Option<String>,
}

pub async fn get_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(q): Query<HomeQuery>,
) -> Result<Response, AppError> {
    let force_home = q.home.as_deref() == Some("1");
    if !force_home {
        if let Some(path) = last_visited::read(&headers) {
            if last_visited::is_safe_path(&path) && target_accessible(&state, &user, &path).await? {
                return Ok(Redirect::to(&path).into_response());
            }
        }
    }
    let (
        sidebar_categories,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let page = WelcomePage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        flash_error: None,
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
        // Require an existing DM room so we never lazily create one on the
        // home redirect. find_dm_room only returns rooms the caller is a
        // member of, so this is also an implicit access check.
        let dm = crate::db::chat::find_dm_room(&state.chat, &user.id, peer_id).await?;
        return Ok(dm.is_some());
    }
    Ok(false)
}
