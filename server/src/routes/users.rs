use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::routes::effective_status;
use crate::state::AppState;
use crate::views::users::{ProfileResult, ProfileSearchFragment};
use crate::views::{html, Html};

const MAX_RESULTS: i64 = 50;

#[derive(Deserialize)]
pub struct UserSearchQuery {
    pub q: Option<String>,
}

/// GET /users/search?q=... - substring match against username and display_name.
///
/// Returns a fragment (no surrounding layout). The sidebar's people-search
/// input uses `hx-target="#main"` with `hx-swap="innerHTML"`, mirroring the
/// message search at /search. An empty query renders an empty results panel
/// without touching the database.
pub async fn get_user_search(
    State(state): State<AppState>,
    AuthUser(viewer): AuthUser,
    Query(UserSearchQuery { q }): Query<UserSearchQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let trimmed = query.trim();

    if trimmed.is_empty() {
        let fragment = ProfileSearchFragment {
            query: trimmed,
            results: &[],
        };
        return html(&fragment);
    }

    let records = db::auth::search_users(&state.auth, trimmed, &viewer.id, MAX_RESULTS).await?;
    let results: Vec<ProfileResult> = records
        .into_iter()
        .map(|r| {
            let is_self = r.id == viewer.id;
            let status = if is_self {
                r.status.clone()
            } else {
                effective_status(&state, &r.id, &r.status)
            };
            ProfileResult {
                id: r.id,
                username: r.username,
                display_name: r.display_name,
                avatar_ext: r.avatar_ext,
                status,
                custom_status: r.custom_status,
                bio: r.bio,
                is_self,
            }
        })
        .collect();

    let fragment = ProfileSearchFragment {
        query: trimmed,
        results: &results,
    };
    html(&fragment)
}
