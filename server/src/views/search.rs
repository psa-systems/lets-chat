use askama::Template;

#[derive(Template)]
#[template(path = "search/results.html")]
pub struct ResultsFragment<'a> {
    pub query: &'a str,
    pub results: &'a [SearchResult],
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
