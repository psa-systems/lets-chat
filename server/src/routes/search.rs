use axum::extract::{Query, State};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::search::{ResultsFragment, SearchResult};
use crate::views::{html, Html};

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub enclave_id: Option<i64>,
}

/// GET /search?q=... - full-text search over messages the caller can read.
///
/// The response is a fragment (no surrounding layout). The sidebar input uses
/// `hx-target="#main"` with `hx-swap="innerHTML"`, so the body of #main is
/// replaced by this template. A direct browser hit (no HX-Request header)
/// receives the same partial - a minor cosmetic trade-off accepted by the
/// plan because users always reach search through the sidebar.
pub async fn get_search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(SearchQuery { q, enclave_id }): Query<SearchQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let trimmed = query.trim();

    // Empty query -> empty results (no DB call). The header still shows the
    // (empty) query string so the UI feels consistent.
    if trimmed.is_empty() {
        let fragment = ResultsFragment {
            query: trimmed,
            results: &[],
        };
        return html(&fragment);
    }

    // Sanitise the user's input into an FTS5-safe MATCH expression. If the
    // input collapses to nothing after stripping FTS operators, treat it as
    // an empty search.
    let fts_query = match db::chat::sanitize_fts_query(trimmed) {
        Some(q) => q,
        None => {
            let fragment = ResultsFragment {
                query: trimmed,
                results: &[],
            };
            return html(&fragment);
        }
    };

    let is_admin = user.role == "admin";
    if let Some(eid) = enclave_id {
        // Membership gate: site admins bypass.
        if !is_admin
            && db::enclave::get_membership(&state.chat, eid, &user.id)
                .await?
                .is_none()
        {
            return Err(AppError::Forbidden);
        }
    }
    let home_scope = enclave_id.is_none();
    let blocked_authors = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let rows: Vec<_> = db::chat::search_messages(
        &state.chat,
        &fts_query,
        None,
        enclave_id,
        home_scope,
        &user.id,
        is_admin,
    )
    .await?
    .into_iter()
    .filter(|r| !blocked_authors.contains(&r.user_id))
    .collect();

    // Build a room_id -> peer_id map so DM hits link to /dm/{peer_id}. Admin
    // search excludes DMs entirely, so this map is only consulted for non-
    // admin callers, but we build it unconditionally for simplicity.
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peer_by_room: HashMap<i64, String> = HashMap::with_capacity(dm_rooms.len());
    for (room, peer_id) in &dm_rooms {
        dm_peer_by_room.insert(room.id, peer_id.clone());
    }

    // Resolve usernames for DM peers so the context label can show "@user"
    // rather than the opaque user_id stored in the DB row.
    let mut username_cache: HashMap<String, String> = HashMap::new();

    let mut results: Vec<SearchResult> = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(peer_id) = dm_peer_by_room.get(&r.room_id) {
            let peer_name = match username_cache.get(peer_id) {
                Some(n) => n.clone(),
                None => {
                    let resolved = db::auth::find_user_by_id(&state.auth, peer_id)
                        .await?
                        .map(|u| u.username)
                        .unwrap_or_else(|| "(unknown)".to_string());
                    username_cache.insert(peer_id.clone(), resolved.clone());
                    resolved
                }
            };
            results.push(SearchResult {
                message_id: r.message_id,
                context_kind: "dm",
                context_id: peer_id.clone(),
                context_label: format!("@{peer_name}"),
                created_at: r.created_at,
                snippet: r.body,
            });
        } else {
            results.push(SearchResult {
                message_id: r.message_id,
                context_kind: "room",
                context_id: r.room_id.to_string(),
                context_label: format!("#{}", r.room_name),
                created_at: r.created_at,
                snippet: r.body,
            });
        }
    }

    let fragment = ResultsFragment {
        query: trimmed,
        results: &results,
    };
    html(&fragment)
}
