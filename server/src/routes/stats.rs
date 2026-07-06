//! LC-536: GET /stats - the viewer's personal activity recap ("wrapped").
//! Self-only: every figure comes from `db::stats::member_stats` scoped to the
//! authenticated user's id, so there is no cross-user surface to gate.

use axum::extract::State;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::stats::StatsPage;
use crate::views::{html, Html};

pub async fn get_stats(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let stats = db::stats::member_stats(&state.chat, &user.id).await?;

    // created_at is a "YYYY-MM-DD HH:MM:SS" UTC string; show just the date.
    let member_since = user
        .created_at
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();

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

    html(&StatsPage {
        user: &user,
        asset_version: &state.asset_version,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        member_since,
        messages_sent: stats.messages_sent,
        active_days: stats.active_days,
        reactions_given: stats.reactions_given,
        reactions_received: stats.reactions_received,
        kudos_received: stats.kudos_received,
        top_rooms: stats.top_rooms,
    })
}
