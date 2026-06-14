use axum::extract::{Query, State};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::search::{ResultsFragment, SearchResult};
use crate::views::{html, Html};

/// Empty popover body. The sidebar's results container uses `empty:hidden`,
/// so an empty body collapses the popover entirely.
fn empty_popover() -> Html {
    Html(String::new())
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub enclave_id: Option<i64>,
    /// LC-268: scope the search to a single room (the room-header search box).
    /// When set, takes precedence over `enclave_id`.
    pub room_id: Option<i64>,
}

/// GET /search?q=... - full-text search over messages the caller can read.
///
/// The response is a popover fragment rendered into the sidebar's
/// `#search-msg-results` target. An empty (or post-sanitisation empty) query
/// returns an empty body so the popover collapses via `empty:hidden`.
pub async fn get_search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(SearchQuery {
        q,
        enclave_id,
        room_id,
    }): Query<SearchQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return Ok(empty_popover());
    }

    // LC-280: pull operator tokens (from:/before:/after:) out of the query; the
    // remainder is the FTS text. Operators refine a text query - a query that is
    // ONLY operators leaves empty text and collapses the popover, same as today.
    let (text, from_name, before, after) = parse_operators(trimmed);

    let fts_query = match db::chat::sanitize_fts_query(&text) {
        Some(q) => q,
        None => return Ok(empty_popover()),
    };

    let is_admin = user.role == "admin";

    // LC-280: resolve from:<username> to an author id; an unknown user can match
    // nothing, so collapse rather than ignore the filter.
    let author_id = if let Some(name) = &from_name {
        match db::auth::find_user_by_username(&state.auth, name).await? {
            Some(rec) => Some(rec.id),
            None => return Ok(empty_popover()),
        }
    } else {
        None
    };
    let filters = db::chat::SearchFilters {
        author_id,
        before,
        after,
    };

    // LC-268: room-scoped search (the room-header box) takes precedence over an
    // enclave scope. Gate on room access (403) then narrow the FTS to that
    // room; the home-scope clause still enforces readability, so the room
    // filter only narrows. When no room is given, fall back to the existing
    // enclave / home behavior.
    let (room_filter, enclave_filter) = if let Some(rid) = room_id {
        if !db::chat::is_room_accessible(&state.chat, rid, &user.id, is_admin).await? {
            return Err(AppError::Forbidden);
        }
        (Some(rid), None)
    } else {
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
        (None, enclave_id)
    };
    let home_scope = enclave_filter.is_none();
    let blocked_authors = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let rows: Vec<_> = db::chat::search_messages_filtered(
        &state.chat,
        &fts_query,
        room_filter,
        enclave_filter,
        home_scope,
        &user.id,
        is_admin,
        &filters,
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

/// LC-280: split a raw query into (free_text, from, before, after). Recognized
/// `key:value` tokens (`from:`, `before:`, `after:`) are pulled out; everything
/// else stays as free text. `before`/`after` values must be `YYYY-MM-DD` or the
/// token is left in the text (so a stray `before:foo` does not silently filter).
/// Last occurrence of a given operator wins.
fn parse_operators(raw: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    let mut text: Vec<&str> = Vec::new();
    let mut from_name = None;
    let mut before = None;
    let mut after = None;
    for tok in raw.split_whitespace() {
        if let Some(v) = tok.strip_prefix("from:") {
            if !v.is_empty() {
                from_name = Some(v.to_string());
                continue;
            }
        } else if let Some(v) = tok.strip_prefix("before:") {
            if is_ymd(v) {
                before = Some(v.to_string());
                continue;
            }
        } else if let Some(v) = tok.strip_prefix("after:") {
            if is_ymd(v) {
                after = Some(v.to_string());
                continue;
            }
        }
        text.push(tok);
    }
    (text.join(" "), from_name, before, after)
}

/// True iff `s` parses as a `YYYY-MM-DD` calendar date.
fn is_ymd(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

#[cfg(test)]
mod tests {
    use super::{is_ymd, parse_operators};

    #[test]
    fn parses_operators_and_leaves_text() {
        let (text, from, before, after) =
            parse_operators("from:alice before:2024-01-15 hello world");
        assert_eq!(text, "hello world");
        assert_eq!(from.as_deref(), Some("alice"));
        assert_eq!(before.as_deref(), Some("2024-01-15"));
        assert_eq!(after, None);
    }

    #[test]
    fn invalid_date_stays_as_text() {
        let (text, _, before, _) = parse_operators("before:nope cat");
        assert_eq!(text, "before:nope cat");
        assert_eq!(before, None);
    }

    #[test]
    fn ymd_validation() {
        assert!(is_ymd("2024-12-31"));
        assert!(!is_ymd("2024-13-01"));
        assert!(!is_ymd("yesterday"));
    }
}
