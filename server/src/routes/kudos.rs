//! LC-526: kudos leaderboard page. Aggregates the `kudos` table over the
//! enclaves the viewer belongs to (so cross-enclave tallies never leak), for
//! the past 30 days. Kudos are given via the `/kudos` slash command
//! (`routes::slash`).

use std::collections::{HashMap, HashSet};

use axum::extract::State;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::kudos::{KudosPage, LeaderRow};
use crate::views::{html, Html};

/// Recap window and per-list size.
const WINDOW: &str = "-30 days";
const LIMIT: i64 = 10;

/// Resolve display labels for a set of user ids in one bulk auth lookup.
async fn labels(state: &AppState, ids: &[&str]) -> Result<HashMap<String, String>, AppError> {
    Ok(db::auth::display_names_for_ids(&state.auth, ids)
        .await?
        .into_iter()
        .map(|(id, (uname, dname))| {
            let label = match dname {
                Some(n) if !n.trim().is_empty() => n,
                _ => uname,
            };
            (id, label)
        })
        .collect())
}

/// GET /kudos - the leaderboard for the viewer's enclaves.
pub async fn get_leaderboard(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let enclave_ids: Vec<i64> = db::enclave::list_enclaves_for_user(&state.chat, &user.id)
        .await?
        .into_iter()
        .map(|e| e.id)
        .collect();

    let receivers = db::kudos::top_receivers(&state.chat, &enclave_ids, WINDOW, LIMIT).await?;
    let givers = db::kudos::top_givers(&state.chat, &enclave_ids, WINDOW, LIMIT).await?;

    // One bulk label lookup covering both lists.
    let mut unique: HashSet<&str> = HashSet::new();
    for l in receivers.iter().chain(givers.iter()) {
        unique.insert(l.user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let names = labels(&state, &ids).await?;
    let row = |i: usize, l: &db::kudos::Leader| LeaderRow {
        rank: i + 1,
        label: names
            .get(&l.user_id)
            .cloned()
            .unwrap_or_else(|| l.user_id.clone()),
        count: l.count,
    };
    let receivers: Vec<LeaderRow> = receivers
        .iter()
        .enumerate()
        .map(|(i, l)| row(i, l))
        .collect();
    let givers: Vec<LeaderRow> = givers.iter().enumerate().map(|(i, l)| row(i, l)).collect();

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

    html(&KudosPage {
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
        receivers,
        givers,
    })
}
