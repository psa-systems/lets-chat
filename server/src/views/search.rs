#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

#[derive(Template)]
#[template(path = "search/results.html")]
pub struct ResultsFragment<'a> {
    pub query: &'a str,
    pub results: &'a [SearchResult],
}

/// LC-312: the saved-searches list shown in the search popover when the input
/// is focused/empty. Each query re-runs by filling the search box client-side.
#[derive(Template)]
#[template(path = "search/saved.html")]
pub struct SavedSearchesFragment {
    pub queries: Vec<String>,
}

pub struct SearchResult {
    pub message_id: i64,
    /// "room" or "dm" - selects the URL prefix.
    pub context_kind: &'static str,
    /// Path segment after the kind: room_id for rooms, peer_id for DMs.
    pub context_id: String,
    /// Human-readable label shown above the snippet (e.g. "#general", "@alice").
    pub context_label: String,
    /// LC-699: the message author, resolved to a display name (`@username`), and
    /// their id for the `/avatars/{id}` thumbnail. `db::chat` only fills
    /// `author_name` with the raw user_id, so `render_results` resolves it.
    pub author_name: String,
    pub author_id: String,
    pub created_at: String,
    /// LC-699: the matched message body, HTML-escaped with the matched query
    /// terms wrapped in `<mark>`. Rendered with `|safe` because it is
    /// pre-escaped server-side; never pass an un-escaped body here.
    pub snippet: String,
}
