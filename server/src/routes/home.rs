use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::OptionalUser;
use crate::error::AppError;
use crate::last_visited;
use crate::models::User;
use crate::state::AppState;
use crate::views::home::{LandingPage, WelcomePage};
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
    OptionalUser(maybe_user): OptionalUser,
    headers: HeaderMap,
    Query(q): Query<HomeQuery>,
) -> Result<Response, AppError> {
    // LC-470: logged-out visitors get the public marketing landing page
    // instead of being bounced to /login. Authenticated users fall through
    // to the existing chat-home behavior below.
    let user = match maybe_user {
        Some(u) => u,
        None => return render_landing(&state),
    };
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
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    // LC-372: pending-invitation count for the welcome quick-action badge.
    let pending_invites = crate::db::enclave::list_invitations_for_user(&state.chat, &user.id)
        .await?
        .len();
    let page = WelcomePage {
        user: &user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        flash_error: None,
        pending_invites,
    };
    let body = html(&page)?;
    Ok(body.into_response())
}

/// LC-470: render the public marketing landing page. The Bunyip issuer host
/// is shown in the "you'll be redirected to ..." note, mirroring the login
/// page; it falls back to "Bunyip" when SSO is not configured.
fn render_landing(state: &AppState) -> Result<Response, AppError> {
    let host = state
        .bunyip_sso
        .as_ref()
        .map(|c| c.config.issuer.host_str().unwrap_or("Bunyip").to_string())
        .unwrap_or_else(|| "Bunyip".to_string());
    let page = LandingPage {
        asset_version: &state.asset_version,
        app_version: crate::version::VERSION,
        git_hash: crate::version::GIT_HASH,
        build_date: crate::version::BUILD_DATE,
        bunyip_issuer_host: &host,
    };
    Ok(html(&page)?.into_response())
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
