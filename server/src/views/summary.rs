//! LC-484: view structs for the AI "catch me up" summaries (threads + channel).

use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;

/// A generated summary, swapped into a container by id (the thread panel's
/// summary slot or the catch-me-up panel body). Carries the regenerate POST
/// URL + the target container id so the regenerate button re-runs in place.
#[derive(Template)]
#[template(path = "room/summary_fragment.html")]
pub struct SummaryFragment {
    pub summary_html: String,
    pub post_url: String,
    pub target_id: String,
}

/// The "Catch me up" drawer, rendered into the shared `#thread-panel` slot.
/// Body starts with a Generate button; the POST swaps the result in.
#[derive(Template)]
#[template(path = "room/catch_me_up_panel.html")]
pub struct CatchMeUpPanel {
    pub room_id: i64,
    pub unread_count: i64,
    pub has_unread: bool,
}
