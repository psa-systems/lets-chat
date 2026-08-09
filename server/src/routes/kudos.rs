//! LC-526: kudos leaderboard page. Aggregates the `kudos` table over the
//! enclaves the viewer belongs to (so cross-enclave tallies never leak), for
//! the past 30 days. Kudos are given via the `/kudos` slash command
//! (`routes::slash`).

use std::collections::{HashMap, HashSet};

use axum::extract::State;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::models::user::UserRecord;
use crate::state::AppState;
use crate::views::kudos::{KudosPage, LeaderRow};
use crate::views::{html, Html};

/// Recap window and per-list size.
const WINDOW: &str = "-30 days";
const LIMIT: i64 = 10;

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

    // LC-526 follow-up: hide users who opted out of the public board.
    let excluded = db::auth::kudos_opted_out_ids(&state.auth).await?;
    let receivers =
        db::kudos::top_receivers(&state.chat, &enclave_ids, WINDOW, LIMIT, &excluded).await?;
    let givers = db::kudos::top_givers(&state.chat, &enclave_ids, WINDOW, LIMIT, &excluded).await?;

    // LC-691: resolve each ranked user's record once (label + avatar + presence)
    // so the leaderboard can show avatars like the other people-listing surfaces.
    // The lists are small (LIMIT each), so per-id lookups are fine.
    let mut records: HashMap<String, UserRecord> = HashMap::new();
    let mut unique: HashSet<&str> = HashSet::new();
    for l in receivers.iter().chain(givers.iter()) {
        unique.insert(l.user_id.as_str());
    }
    for id in unique {
        if let Some(rec) = db::auth::find_user_by_id(&state.auth, id).await? {
            records.insert(id.to_string(), rec);
        }
    }
    let row = |i: usize, l: &db::kudos::Leader| {
        let rec = records.get(&l.user_id);
        let label = rec
            .and_then(|r| r.display_name.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| rec.map(|r| format!("@{}", r.username)))
            .unwrap_or_else(|| l.user_id.clone());
        let status = super::effective_status(
            &state,
            &l.user_id,
            rec.map(|r| r.status.as_str()).unwrap_or("offline"),
        );
        LeaderRow {
            rank: i + 1,
            label,
            count: l.count,
            user_id: l.user_id.clone(),
            avatar_ext: rec.and_then(|r| r.avatar_ext.clone()),
            status,
            custom_status: rec.and_then(|r| r.custom_status.clone()),
        }
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
