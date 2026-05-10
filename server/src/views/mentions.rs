use askama::Template;

/// One row in the autocomplete dropdown rendered by
/// `partials/mention_popover.html`.
#[derive(Clone)]
pub struct MentionSuggestion {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
}

#[derive(Template)]
#[template(path = "partials/mention_popover.html")]
pub struct MentionPopoverFragment<'a> {
    pub results: &'a [MentionSuggestion],
}
