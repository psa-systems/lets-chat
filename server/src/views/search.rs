#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

#[derive(Template)]
#[template(path = "search/results.html")]
pub struct ResultsFragment<'a> {
    pub query: &'a str,
    pub results: &'a [SearchResult],
    /// LC-938: the id of the container this fragment is swapped into
    /// (`lc-room-search-results` or `sidebar-search-results`). The room-header
    /// popover and the sidebar popover coexist in the DOM at once, so every id
    /// the fragment renders is derived from this to stay unique document-wide.
    pub container_id: &'static str,
    /// LC-938: which scope the underlying query actually searched, set by the
    /// route from whether `room_id` / `enclave_id` was present. Selects the
    /// results-header label instead of a hardcoded "This room".
    pub scope_key: &'static str,
}

/// LC-312: the saved-searches list shown in the search popover when the input
/// is focused/empty. Each query re-runs by filling the search box client-side.
#[derive(Template)]
#[template(path = "search/saved.html")]
pub struct SavedSearchesFragment {
    pub queries: Vec<String>,
    /// LC-938: see `ResultsFragment::container_id`.
    pub container_id: &'static str,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// All `id="..."` attribute values in a rendered fragment.
    fn ids(html: &str) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        let mut rest = html;
        while let Some(pos) = rest.find("id=\"") {
            rest = &rest[pos + 4..];
            let end = rest.find('"').expect("unterminated id attribute");
            set.insert(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        set
    }

    fn sample_result() -> SearchResult {
        SearchResult {
            message_id: 1,
            context_kind: "room",
            context_id: "1".to_string(),
            context_label: "#general".to_string(),
            author_name: "@alice".to_string(),
            author_id: "u1".to_string(),
            created_at: "2024-01-01".to_string(),
            snippet: "hello".to_string(),
        }
    }

    /// LC-938: the room-header and sidebar popovers coexist in the DOM, so
    /// rendering the same results fragment for both container ids must never
    /// produce a shared id.
    #[test]
    fn results_fragment_ids_disjoint_across_containers() {
        let results = [sample_result()];

        let room_html = ResultsFragment {
            query: "hello",
            results: &results,
            container_id: "lc-room-search-results",
            scope_key: "search-scope-room",
        }
        .render()
        .unwrap();
        let sidebar_html = ResultsFragment {
            query: "hello",
            results: &results,
            container_id: "sidebar-search-results",
            scope_key: "search-scope-all",
        }
        .render()
        .unwrap();

        let room_ids = ids(&room_html);
        let sidebar_ids = ids(&sidebar_html);
        assert!(!room_ids.is_empty());
        assert!(
            room_ids.is_disjoint(&sidebar_ids),
            "shared ids: {:?}",
            room_ids.intersection(&sidebar_ids).collect::<Vec<_>>()
        );
    }
}
