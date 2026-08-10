use axum::extract::{Query, State};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::search::{ResultsFragment, SavedSearchesFragment, SearchResult};
use crate::views::{html, Html};

/// Empty popover body. The sidebar's results container uses `empty:hidden`,
/// so an empty body collapses the popover entirely.
fn empty_popover() -> Html {
    Html(String::new())
}

/// LC-312: when the search box is focused/empty, show the caller's saved
/// searches instead of an empty popover. Falls back to empty (collapsed) when
/// they have none, preserving the old behavior for users with no saved searches.
async fn saved_or_empty(state: &AppState, user_id: &str) -> Result<Html, AppError> {
    let queries = db::saved_searches::list(&state.chat, user_id).await?;
    if queries.is_empty() {
        return Ok(empty_popover());
    }
    html(&SavedSearchesFragment { queries })
}

#[derive(Deserialize)]
pub struct SaveForm {
    #[serde(default)]
    pub query: String,
}

/// POST /searches - save the current query (cap-enforced). Returns a small
/// "Saved" confirmation that replaces the save form in place (target=self), so
/// it works inside any results popover regardless of its container id.
pub async fn post_save(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SaveForm>,
) -> Result<Html, AppError> {
    let query = form.query.trim();
    if !query.is_empty()
        && db::saved_searches::count(&state.chat, &user.id).await?
            < db::saved_searches::MAX_SAVED_SEARCHES as i64
    {
        db::saved_searches::add(&state.chat, &user.id, query).await?;
    }
    let label = crate::i18n::translate_current("search-saved");
    Ok(Html(format!(
        r#"<span class="text-xs text-success-surface-content">{}</span>"#,
        crate::views::room::html_escape(&label)
    )))
}

/// POST /searches/delete - remove a saved query; re-render the saved list.
pub async fn post_delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SaveForm>,
) -> Result<Html, AppError> {
    db::saved_searches::remove(&state.chat, &user.id, form.query.trim()).await?;
    saved_or_empty(&state, &user.id).await
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub enclave_id: Option<i64>,
    /// LC-268: scope the search to a single room (the room-header search box).
    /// When set, takes precedence over `enclave_id`.
    pub room_id: Option<i64>,
    /// LC-549: opt-in semantic mode (the room-header "Semantic" toggle). Ranks by
    /// embedding similarity instead of FTS keywords. Requires `room_id` and a
    /// configured embeddings endpoint; otherwise the search degrades to FTS.
    #[serde(default)]
    pub semantic: Option<String>,
}

/// LC-549: how many semantic hits to surface, and the minimum cosine similarity
/// to count as a hit (drop near-orthogonal noise).
const SEMANTIC_LIMIT: usize = 20;
const SEMANTIC_MIN_SIMILARITY: f32 = 0.15;

/// Truthy check for a checkbox-style query flag ("1" / "on" / "true").
fn flag_on(v: &Option<String>) -> bool {
    matches!(v.as_deref(), Some("1") | Some("on") | Some("true"))
}

/// LC-549: semantic ranking over one room's stored embeddings. Returns `None`
/// (so the caller degrades to FTS) when embeddings are unavailable or the query
/// cannot be embedded; `Some(rows)` otherwise (possibly empty). Room access is
/// enforced by the caller before this runs.
async fn semantic_rows(
    state: &AppState,
    room_id: i64,
    query_text: &str,
) -> Option<Vec<crate::models::SearchResult>> {
    let client = state.embedding_client.clone()?;
    let query_vec = match client.embed(query_text).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "semantic search embed failed; falling back to FTS");
            return None;
        }
    };
    let candidates = db::message_embeddings::list_for_room(&state.chat, room_id, None)
        .await
        .ok()?;
    let mut scored: Vec<(i64, f32)> = candidates
        .into_iter()
        .map(|c| {
            (
                c.message_id,
                crate::embeddings::cosine_similarity(&query_vec, &c.vec),
            )
        })
        .filter(|(_, s)| *s >= SEMANTIC_MIN_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(SEMANTIC_LIMIT);
    let ids: Vec<i64> = scored.into_iter().map(|(id, _)| id).collect();
    db::chat::search_results_for_ids(&state.chat, room_id, &ids)
        .await
        .ok()
}

/// GET /search?q=... - full-text search over messages the caller can read.
///
/// The response is a popover fragment rendered into the sidebar's unified
/// `#sidebar-search-results` target (LC-369). An empty (or post-sanitisation
/// empty) query returns an empty body so the popover collapses via
/// `empty:hidden`.
pub async fn get_search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(SearchQuery {
        q,
        enclave_id,
        room_id,
        semantic,
    }): Query<SearchQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let trimmed = query.trim();

    if trimmed.is_empty() {
        // LC-312: an empty query (the box was focused or cleared) shows the
        // caller's saved searches, scoped to the sidebar message-search box.
        // The room-header box passes room_id, so restrict the saved list to the
        // unscoped sidebar search to avoid surfacing it in the room header.
        if room_id.is_none() && enclave_id.is_none() {
            return saved_or_empty(&state, &user.id).await;
        }
        return Ok(empty_popover());
    }

    // LC-280: pull operator tokens (from:/before:/after:) out of the query; the
    // remainder is the FTS text. Operators refine a text query - a query that is
    // ONLY operators leaves empty text and collapses the popover, same as today.
    let parsed = parse_operators(trimmed);
    let is_admin = user.role == "admin";

    // LC-549: opt-in semantic mode. Room-scoped only (the room-header toggle);
    // access-check the room, then rank embeddings. On any miss - embeddings not
    // configured, the query cannot be embedded, or a DB error - fall through to
    // the FTS keyword path below so search always returns something.
    if flag_on(&semantic) && state.embeddings_available() {
        if let Some(rid) = room_id {
            if !db::chat::is_room_accessible(&state.chat, rid, &user.id, is_admin).await? {
                return Err(AppError::Forbidden);
            }
            // LC-679: semantic ranking is behind the runtime flag + role gate. An
            // ungated viewer silently falls through to the FTS keyword path below
            // rather than getting a 403 - search is a core feature, and the only
            // difference here is the embeddings-powered ranking.
            if super::ai_gate::can_use_embeddings_in_room(&state, rid, &user).await? {
                let query_text = if parsed.text.trim().is_empty() {
                    trimmed
                } else {
                    parsed.text.as_str()
                };
                if let Some(rows) = semantic_rows(&state, rid, query_text).await {
                    let blocked =
                        db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
                    let rows: Vec<_> = rows
                        .into_iter()
                        .filter(|r| !blocked.contains(&r.user_id))
                        .collect();
                    return render_results(&state, &user, trimmed, query_text, rows).await;
                }
            }
        }
    }

    let fts_query = match db::chat::sanitize_fts_query(&parsed.text) {
        Some(q) => q,
        None => return Ok(empty_popover()),
    };

    // LC-280: resolve from:<username> to an author id; an unknown user can match
    // nothing, so collapse rather than ignore the filter.
    let author_id = if let Some(name) = &parsed.from_name {
        match db::auth::find_user_by_username(&state.auth, name).await? {
            Some(rec) => Some(rec.id),
            None => return Ok(empty_popover()),
        }
    } else {
        None
    };
    let filters = db::chat::SearchFilters {
        author_id,
        before: parsed.before,
        after: parsed.after,
        has_file: parsed.has_file,
        has_link: parsed.has_link,
        in_thread: parsed.in_thread,
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

    render_results(&state, &user, trimmed, &parsed.text, rows).await
}

/// Turn ranked `models::SearchResult` rows (from FTS or LC-549 semantic ranking)
/// into the rendered popover fragment, mapping DM hits to `/dm/{peer}` links and
/// room hits to `#room` labels. Shared by both search paths so the DM/label
/// logic stays in one place.
async fn render_results(
    state: &AppState,
    user: &crate::models::User,
    trimmed: &str,
    highlight: &str,
    rows: Vec<crate::models::SearchResult>,
) -> Result<Html, AppError> {
    // Build a room_id -> peer_id map so DM hits link to /dm/{peer_id}. Admin
    // search excludes DMs entirely, so this map is only consulted for non-
    // admin callers, but we build it unconditionally for simplicity.
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peer_by_room: HashMap<i64, String> = HashMap::with_capacity(dm_rooms.len());
    for (room, peer_id) in &dm_rooms {
        dm_peer_by_room.insert(room.id, peer_id.clone());
    }

    // Resolve usernames for DM peers (the context label) and message authors
    // (the row byline). `db::chat` only fills `author_name` with the raw
    // user_id, so we resolve it here, caching one lookup per distinct id.
    let mut username_cache: HashMap<String, String> = HashMap::new();

    // LC-699: highlight the matched query terms in each snippet.
    let terms = highlight_terms(highlight);

    let mut results: Vec<SearchResult> = Vec::with_capacity(rows.len());
    for r in rows {
        let author_name = resolve_username(state, &mut username_cache, &r.user_id).await?;
        let snippet = highlight_body(&r.body, &terms);
        if let Some(peer_id) = dm_peer_by_room.get(&r.room_id) {
            let peer_name = resolve_username(state, &mut username_cache, peer_id).await?;
            results.push(SearchResult {
                message_id: r.message_id,
                context_kind: "dm",
                context_id: peer_id.clone(),
                context_label: format!("@{peer_name}"),
                author_name: format!("@{author_name}"),
                author_id: r.user_id,
                created_at: r.created_at,
                snippet,
            });
        } else {
            results.push(SearchResult {
                message_id: r.message_id,
                context_kind: "room",
                context_id: r.room_id.to_string(),
                context_label: format!("#{}", r.room_name),
                author_name: format!("@{author_name}"),
                author_id: r.user_id,
                created_at: r.created_at,
                snippet,
            });
        }
    }

    let fragment = ResultsFragment {
        query: trimmed,
        results: &results,
    };
    html(&fragment)
}

/// Resolve a user_id to its username, caching each distinct lookup. Unknown ids
/// (deleted accounts) fall back to "(unknown)".
async fn resolve_username(
    state: &AppState,
    cache: &mut HashMap<String, String>,
    user_id: &str,
) -> Result<String, AppError> {
    if let Some(n) = cache.get(user_id) {
        return Ok(n.clone());
    }
    let resolved = db::auth::find_user_by_id(&state.auth, user_id)
        .await?
        .map(|u| u.username)
        .unwrap_or_else(|| "(unknown)".to_string());
    cache.insert(user_id.to_string(), resolved.clone());
    Ok(resolved)
}

/// LC-699: the distinct free-text words to highlight, ASCII-lowercased and
/// length-filtered so single-character noise ("a", punctuation) does not mark
/// half the snippet. Operators were already stripped before this is called.
fn highlight_terms(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for w in text.split_whitespace() {
        let w: String = w
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        if w.len() >= 2 && !terms.contains(&w) {
            terms.push(w);
        }
    }
    terms
}

/// LC-699: HTML-escape `body` and wrap case-insensitive matches of any term in
/// `<mark>`. Matching is done on an ASCII-lowercased copy so byte offsets stay
/// aligned with the original (ASCII lowercasing never changes byte length), so
/// multi-byte UTF-8 is never sliced mid-character. The result is safe HTML.
fn highlight_body(body: &str, terms: &[String]) -> String {
    use crate::views::room::html_escape;
    if terms.is_empty() {
        return html_escape(body);
    }
    let lower = body.to_ascii_lowercase();
    let mut out = String::with_capacity(body.len() + 16);
    let mut i = 0;
    while i < body.len() {
        let mut matched = false;
        for term in terms {
            if lower[i..].starts_with(term.as_str()) {
                let end = i + term.len();
                out.push_str("<mark>");
                out.push_str(&html_escape(&body[i..end]));
                out.push_str("</mark>");
                i = end;
                matched = true;
                break;
            }
        }
        if !matched {
            let ch_len = body[i..].chars().next().map_or(1, |c| c.len_utf8());
            out.push_str(&html_escape(&body[i..i + ch_len]));
            i += ch_len;
        }
    }
    out
}

/// LC-280 + LC-530: a raw query split into free text plus operator refinements.
#[derive(Default)]
struct ParsedQuery {
    text: String,
    from_name: Option<String>,
    before: Option<String>,
    after: Option<String>,
    has_file: bool,
    has_link: bool,
    in_thread: bool,
}

/// Split a raw query into free text plus operator refinements. Recognized
/// `key:value` tokens (`from:`, `before:`, `after:`, `has:file`, `has:link`,
/// `in:thread`) are pulled out; everything else stays as free text. Date values
/// must be `YYYY-MM-DD`, and `has:`/`in:` values must be known, or the token is
/// left in the text (so a stray `before:foo` / `has:whatever` does not silently
/// filter). Last occurrence of a given operator wins.
fn parse_operators(raw: &str) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    let mut text: Vec<&str> = Vec::new();
    for tok in raw.split_whitespace() {
        if let Some(v) = tok.strip_prefix("from:") {
            if !v.is_empty() {
                out.from_name = Some(v.to_string());
                continue;
            }
        } else if let Some(v) = tok.strip_prefix("before:") {
            if is_ymd(v) {
                out.before = Some(v.to_string());
                continue;
            }
        } else if let Some(v) = tok.strip_prefix("after:") {
            if is_ymd(v) {
                out.after = Some(v.to_string());
                continue;
            }
        } else if tok == "has:file" {
            out.has_file = true;
            continue;
        } else if tok == "has:link" {
            out.has_link = true;
            continue;
        } else if tok == "in:thread" {
            out.in_thread = true;
            continue;
        }
        text.push(tok);
    }
    out.text = text.join(" ");
    out
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
        let p = parse_operators("from:alice before:2024-01-15 hello world");
        assert_eq!(p.text, "hello world");
        assert_eq!(p.from_name.as_deref(), Some("alice"));
        assert_eq!(p.before.as_deref(), Some("2024-01-15"));
        assert_eq!(p.after, None);
    }

    #[test]
    fn invalid_date_stays_as_text() {
        let p = parse_operators("before:nope cat");
        assert_eq!(p.text, "before:nope cat");
        assert_eq!(p.before, None);
    }

    #[test]
    fn parses_has_and_in_operators() {
        let p = parse_operators("has:file has:link in:thread report");
        assert_eq!(p.text, "report");
        assert!(p.has_file);
        assert!(p.has_link);
        assert!(p.in_thread);
    }

    #[test]
    fn unknown_has_in_value_stays_as_text() {
        let p = parse_operators("has:whatever in:box cat");
        assert_eq!(p.text, "has:whatever in:box cat");
        assert!(!p.has_file);
        assert!(!p.has_link);
        assert!(!p.in_thread);
    }

    #[test]
    fn ymd_validation() {
        assert!(is_ymd("2024-12-31"));
        assert!(!is_ymd("2024-13-01"));
        assert!(!is_ymd("yesterday"));
    }
}
