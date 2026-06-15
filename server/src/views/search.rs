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
    pub created_at: String,
    /// Plain message body. Askama escapes this on render; the FTS layer does
    /// not produce pre-formatted `<mark>` HTML, so no `|safe` filter is used.
    pub snippet: String,
}
