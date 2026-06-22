//! HTMX endpoints for per-user starred rooms (LC-80).
//!
//! `POST /rooms/{room_id}/star` toggles the star. The accessibility
//! check (`is_room_accessible`) keeps the endpoint from disclosing
//! private-room existence; without it, a probe by room id would
//! distinguish "exists but not yours" from "doesn't exist" by the
//! handler's success/no-op behaviour.
//!
//! `PATCH /sidebar/stars/positions` accepts a comma-separated `ids`
//! list and rewrites per-row positions for the calling user.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;
use crate::views::ws_fragments::SidebarUpdateFragment;
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct PositionsForm {
    #[serde(default)]
    pub ids: String,
}

impl PositionsForm {
    fn parsed_ids(&self) -> Result<Vec<i64>, AppError> {
        self.ids
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<i64>()
                    .map_err(|_| AppError::BadRequest(format!("invalid id: {s}")))
            })
            .collect()
    }
}

/// POST /rooms/{room_id}/star - toggle star.
pub async fn post_toggle(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    let currently = db::starred_rooms::is_starred(&state.auth, &user.id, room_id).await?;
    if currently {
        db::starred_rooms::unstar(&state.auth, &user.id, room_id).await?;
    } else {
        db::starred_rooms::star(&state.auth, &user.id, room_id).await?;
    }
    render_sidebar_fragment(&state, &user, &headers).await
}

/// PATCH /sidebar/stars/positions - reorder starred rooms.
pub async fn patch_positions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Form(form): Form<PositionsForm>,
) -> Result<Html, AppError> {
    let ids = form.parsed_ids()?;
    db::starred_rooms::set_positions(&state.auth, &user.id, &ids).await?;
    render_sidebar_fragment(&state, &user, &headers).await
}

async fn render_sidebar_fragment(
    state: &AppState,
    user: &User,
    headers: &HeaderMap,
) -> Result<Html, AppError> {
    let current_enclave = super::current_enclave_for_sidebar(state, headers).await;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_sidebar(state, user, current_enclave).await?;
    let fragment = SidebarUpdateFragment {
        user,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
    };
    html(&fragment)
}
